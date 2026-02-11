use std::io::Write;
use zenith_bundler::{
    bundle_page, BuildMode, BundleError, BundleOptions, BundlePlan, CompilerOutput,
};

/// Create a temp .zen file with the given content.
fn create_temp_zen(content: &str) -> tempfile::NamedTempFile {
    let mut file = tempfile::Builder::new()
        .suffix(".zen")
        .tempfile()
        .expect("Failed to create temp file");
    file.write_all(content.as_bytes())
        .expect("Failed to write temp file");
    file
}

// ============================================================================
// M0: Smoke tests — bundle_page returns a valid BundleResult
// ============================================================================

#[tokio::test]
async fn bundle_simple_page() {
    let file = create_temp_zen("<h1>{title}</h1>");
    let plan = BundlePlan {
        page_path: file.path().to_string_lossy().to_string(),
        out_dir: None,
        mode: BuildMode::Dev,
    };
    let opts = BundleOptions {
        strict: false,
        ..Default::default()
    };

    let result = bundle_page(plan, opts).await.unwrap();

    // Entry JS must contain both exports
    assert!(result.entry_js.contains("__zenith_html"));
    assert!(result.entry_js.contains("__zenith_expr"));
    assert_eq!(result.expressions, vec!["title"]);
}

#[tokio::test]
async fn bundle_static_page_no_expressions() {
    let file = create_temp_zen("<div>Hello World</div>");
    let plan = BundlePlan {
        page_path: file.path().to_string_lossy().to_string(),
        out_dir: None,
        mode: BuildMode::Dev,
    };
    let opts = BundleOptions::default();

    let result = bundle_page(plan, opts).await.unwrap();

    assert!(result.expressions.is_empty());
    assert!(result.entry_js.contains("__zenith_html"));
}

#[tokio::test]
async fn bundle_multiple_expressions() {
    let file = create_temp_zen(
        r#"<div id="app"><h1>{title}</h1><button on:click={increment}>Count: {count}</button></div>"#,
    );
    let plan = BundlePlan {
        page_path: file.path().to_string_lossy().to_string(),
        out_dir: None,
        mode: BuildMode::Dev,
    };
    let opts = BundleOptions::default();

    let result = bundle_page(plan, opts).await.unwrap();

    assert_eq!(result.expressions, vec!["title", "increment", "count"]);
}

// ============================================================================
// M1: Strict mode validation
// ============================================================================

#[tokio::test]
async fn strict_mode_matching_metadata_passes() {
    let file = create_temp_zen("<h1>{title}</h1>");
    let plan = BundlePlan {
        page_path: file.path().to_string_lossy().to_string(),
        out_dir: None,
        mode: BuildMode::Dev,
    };

    // Provide matching metadata
    let metadata = CompilerOutput {
        html: String::new(), // HTML not used for expression comparison
        expressions: vec!["title".into()],
    };

    let opts = BundleOptions {
        metadata: Some(metadata),
        strict: true,
        ..Default::default()
    };

    let result = bundle_page(plan, opts).await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn strict_mode_mismatched_count_fails() {
    let file = create_temp_zen("<h1>{title}</h1>");
    let plan = BundlePlan {
        page_path: file.path().to_string_lossy().to_string(),
        out_dir: None,
        mode: BuildMode::Dev,
    };

    // Provide wrong metadata (expects 2 expressions)
    let metadata = CompilerOutput {
        html: String::new(),
        expressions: vec!["title".into(), "extra".into()],
    };

    let opts = BundleOptions {
        metadata: Some(metadata),
        strict: true,
        ..Default::default()
    };

    let result = bundle_page(plan, opts).await;
    assert!(result.is_err());
    match result.unwrap_err() {
        BundleError::ExpressionMismatch { expected, got } => {
            assert_eq!(expected, 2);
            assert_eq!(got, 1);
        }
        e => panic!("Expected ExpressionMismatch, got: {:?}", e),
    }
}

#[tokio::test]
async fn strict_mode_mismatched_content_fails() {
    let file = create_temp_zen("<h1>{title}</h1>");
    let plan = BundlePlan {
        page_path: file.path().to_string_lossy().to_string(),
        out_dir: None,
        mode: BuildMode::Dev,
    };

    // Right count, wrong content
    let metadata = CompilerOutput {
        html: String::new(),
        expressions: vec!["wrong_name".into()],
    };

    let opts = BundleOptions {
        metadata: Some(metadata),
        strict: true,
        ..Default::default()
    };

    let result = bundle_page(plan, opts).await;
    assert!(result.is_err());
    match result.unwrap_err() {
        BundleError::ExpressionContentMismatch { index, .. } => {
            assert_eq!(index, 0);
        }
        e => panic!("Expected ExpressionContentMismatch, got: {:?}", e),
    }
}

// ============================================================================
// M1: File not found error
// ============================================================================

#[tokio::test]
async fn bundle_nonexistent_file_fails() {
    let plan = BundlePlan {
        page_path: "/nonexistent/path/page.zen".into(),
        out_dir: None,
        mode: BuildMode::Dev,
    };
    let opts = BundleOptions::default();

    let result = bundle_page(plan, opts).await;
    assert!(result.is_err());
    match result.unwrap_err() {
        BundleError::IoError(_) => {}
        e => panic!("Expected IoError, got: {:?}", e),
    }
}

// ============================================================================
// M1: Diagnostics
// ============================================================================

#[tokio::test]
async fn bundle_emits_diagnostics() {
    let file = create_temp_zen("<p>{x}</p>");
    let plan = BundlePlan {
        page_path: file.path().to_string_lossy().to_string(),
        out_dir: None,
        mode: BuildMode::Dev,
    };
    let opts = BundleOptions::default();

    let result = bundle_page(plan, opts).await.unwrap();

    // Should have at least "Bundle started" and "Bundle complete" diagnostics
    assert!(result.diagnostics.len() >= 2);
    assert!(result
        .diagnostics
        .iter()
        .any(|d| d.message.contains("Bundle started")));
    assert!(result
        .diagnostics
        .iter()
        .any(|d| d.message.contains("Bundle complete")));
}

// ============================================================================
// M1: Expression never mutated
// ============================================================================

#[tokio::test]
async fn expressions_never_mutated() {
    let file = create_temp_zen(r#"<div title={myVar}><span>{other_var}</span></div>"#);
    let plan = BundlePlan {
        page_path: file.path().to_string_lossy().to_string(),
        out_dir: None,
        mode: BuildMode::Dev,
    };
    let opts = BundleOptions::default();

    let result = bundle_page(plan, opts).await.unwrap();

    // Expressions must be exact strings from the source
    assert_eq!(result.expressions, vec!["myVar", "other_var"]);
    // The JS must contain exactly these strings
    assert!(result.entry_js.contains("\"myVar\""));
    assert!(result.entry_js.contains("\"other_var\""));
}

// ============================================================================
// M1: No post-concat
// ============================================================================

#[tokio::test]
async fn no_post_concat_in_bundle() {
    let file = create_temp_zen("<div>{x}</div>");
    let plan = BundlePlan {
        page_path: file.path().to_string_lossy().to_string(),
        out_dir: None,
        mode: BuildMode::Dev,
    };
    let opts = BundleOptions::default();

    let result = bundle_page(plan, opts).await.unwrap();

    // The entry JS must be a valid module — check for export statements
    assert!(result.entry_js.contains("export"));
    // Must NOT contain bare script injection patterns
    assert!(!result.entry_js.contains("<script"));
    assert!(!result.entry_js.contains("document.write"));
}
