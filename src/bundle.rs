//! Core bundling logic.
//!
//! This module orchestrates the full bundle pipeline:
//! 1. Read and compile the `.zen` source via the ZenithLoader plugin
//! 2. Run through Rolldown for import resolution and graph building
//! 3. Validate output against metadata
//! 4. Return sealed BundleResult
//!
//! **Single emission engine.** All builds go through Rolldown.
//! There is one graph, one emission flow, one source of truth.
//! No inline bypass is permitted — determinism requires a unified pipeline.

use std::path::Path;
use std::sync::Arc;

use rolldown::{BundlerBuilder, BundlerOptions, InputItem};
use rolldown_common::OutputFormat;

use crate::plugin::zenith_loader::{ZenithLoader, ZenithLoaderConfig};
use crate::utils;
use crate::{
    BuildMode, BundleError, BundleOptions, BundlePlan, BundleResult, Diagnostic, DiagnosticLevel,
};

// ---------------------------------------------------------------------------
// Single emission engine — all builds go through Rolldown
// ---------------------------------------------------------------------------

/// Execute the bundle pipeline using Rolldown as the single emission engine.
///
/// This creates a ZenithLoader plugin, wires it into Rolldown via
/// `BundlerBuilder`, runs the full build, and validates the output.
///
/// **Invariant:** There is no alternative codepath. Every build —
/// single-page, multi-page, dev, prod — runs through this function.
pub async fn execute_bundle(
    plan: BundlePlan,
    opts: BundleOptions,
) -> Result<BundleResult, BundleError> {
    let mut diagnostics: Vec<Diagnostic> = Vec::new();

    let page_id = utils::canonicalize_page_id(&plan.page_path);

    // Pre-build: verify source file exists (clean IoError)
    if !Path::new(&plan.page_path).exists() {
        return Err(BundleError::IoError(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("Source file not found: {}", plan.page_path),
        )));
    }

    diagnostics.push(Diagnostic {
        level: DiagnosticLevel::Info,
        message: format!(
            "Bundle started for page: {} (id: {})",
            plan.page_path, page_id
        ),
        context: None,
    });

    // Create the loader plugin
    let loader = ZenithLoader::new(ZenithLoaderConfig {
        components: opts.components.clone(),
        metadata: opts.metadata.clone(),
        strict: opts.strict,
        is_dev: plan.mode == BuildMode::Dev,
    });

    let compiled_outputs = loader.compiled_outputs();
    let css_cache = loader.css_cache();

    // Configure Rolldown — single-entry, ESM, browser
    let rolldown_options = BundlerOptions {
        input: Some(vec![InputItem {
            name: Some("index".into()),
            import: plan.page_path.clone(),
        }]),
        format: Some(OutputFormat::Esm),
        platform: Some(rolldown_common::Platform::Browser),
        minify: if opts.minify.unwrap_or(plan.mode == BuildMode::Prod) {
            Some(Default::default())
        } else {
            None
        },
        ..Default::default()
    };

    // Build bundler with plugin
    let mut bundler = BundlerBuilder::default()
        .with_options(rolldown_options)
        .with_plugins(vec![Arc::new(loader)])
        .build()
        .map_err(|e| BundleError::BuildError(format!("Rolldown init failed: {:?}", e)))?;

    // Run the bundling pass
    let bundle_output = bundler
        .generate()
        .await
        .map_err(|e| BundleError::BuildError(format!("Rolldown build failed: {:?}", e)))?;

    // Close the bundler
    bundler
        .close()
        .await
        .map_err(|e| BundleError::BuildError(format!("Rolldown close failed: {:?}", e)))?;

    // Extract the entry chunk
    let entry_js = bundle_output
        .assets
        .iter()
        .find_map(|asset| match asset {
            rolldown_common::Output::Chunk(chunk) => Some(chunk.code.clone()),
            _ => None,
        })
        .ok_or_else(|| BundleError::BuildError("No entry chunk in Rolldown output".into()))?;

    // Strip non-deterministic comments (Rolldown emits //#region with absolute paths)
    // Also normalizes line endings to \n
    let entry_js = entry_js
        .lines()
        .filter(|line| !line.starts_with("//#region") && !line.starts_with("//#endregion"))
        .collect::<Vec<_>>()
        .join("\n");

    // Get compiled output for the page (stored by the plugin during load)
    let compiled = compiled_outputs
        .get(&plan.page_path)
        .map(|entry| entry.value().clone())
        .unwrap_or_default();

    let expressions = compiled.expressions.clone();

    // Post-build strict validation
    if opts.strict {
        // 1. Verify expressions match metadata
        if let Some(ref metadata) = opts.metadata {
            utils::validate_expressions(&expressions, &metadata.expressions)?;
        }

        // 2. Verify HTML contains required placeholders
        if !expressions.is_empty() {
            if let Err(diags) = utils::validate_placeholders(&compiled.html, expressions.len()) {
                return Err(BundleError::ValidationError(
                    diags
                        .iter()
                        .map(|d| d.message.clone())
                        .collect::<Vec<_>>()
                        .join("; "),
                ));
            }
        }
    }

    // Collect CSS
    let css = css_cache.get(&page_id);

    diagnostics.push(Diagnostic {
        level: DiagnosticLevel::Info,
        message: format!(
            "Bundle complete: {} expressions, {} bytes JS, {} bytes CSS",
            expressions.len(),
            entry_js.len(),
            css.as_ref().map_or(0, |c| c.len()),
        ),
        context: None,
    });

    // Write to disk if requested
    if opts.write_to_disk {
        let out_dir = plan
            .out_dir
            .unwrap_or_else(|| Path::new("dist").to_path_buf());
        let pages_dir = out_dir.join("pages");
        tokio::fs::create_dir_all(&pages_dir).await?;

        let js_path = pages_dir.join(format!("{}.js", page_id));
        tokio::fs::write(&js_path, &entry_js).await?;

        if let Some(ref css_content) = css {
            let css_path = pages_dir.join(format!("{}.css", page_id));
            tokio::fs::write(&css_path, css_content).await?;
        }

        diagnostics.push(Diagnostic {
            level: DiagnosticLevel::Info,
            message: format!("Written to {}", pages_dir.display()),
            context: None,
        });
    }

    Ok(BundleResult {
        entry_js,
        css,
        expressions,
        diagnostics,
    })
}
