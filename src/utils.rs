//! Utility functions for the bundler.
//!
//! - Virtual module ID construction and parsing
//! - JS string escaping (injection-safe)
//! - Post-build validation helpers

use regex::Regex;

use crate::{BundleError, CompilerOutput, Diagnostic, DiagnosticLevel};

// ---------------------------------------------------------------------------
// Virtual Module IDs
// ---------------------------------------------------------------------------

/// Prefix for all Zenith virtual modules.
/// The `\0` prefix prevents filesystem resolution collisions.
pub const VIRTUAL_PREFIX: &str = "\0zenith:";

/// Create the virtual entry module ID for a page.
pub fn virtual_entry_id(page_id: &str) -> String {
    format!("\0zenith:entry:{}", page_id)
}

/// Create the virtual CSS module ID for a page.
pub fn virtual_css_id(page_id: &str) -> String {
    format!("\0zenith:css:{}", page_id)
}

/// Create the virtual page-script module ID.
pub fn virtual_page_script_id(page_id: &str) -> String {
    format!("\0zenith:page-script:{}", page_id)
}

/// Extract the page ID from a virtual module ID.
/// Returns `None` if the ID doesn't match the expected pattern.
pub fn extract_page_id(virtual_id: &str) -> Option<&str> {
    if let Some(rest) = virtual_id.strip_prefix("\0zenith:entry:") {
        Some(rest)
    } else if let Some(rest) = virtual_id.strip_prefix("\0zenith:css:") {
        Some(rest)
    } else if let Some(rest) = virtual_id.strip_prefix("\0zenith:page-script:") {
        Some(rest)
    } else {
        None
    }
}

/// Check if a module ID is a Zenith virtual module.
pub fn is_virtual(id: &str) -> bool {
    id.starts_with(VIRTUAL_PREFIX)
}

/// Check if a file path is a `.zen` file.
pub fn is_zen_file(path: &str) -> bool {
    path.ends_with(".zen")
}

/// Check if a module ID is a Zenith internal virtual module.
/// These use the `\0zenith:` prefix and must never be user-resolvable.
pub fn is_zenith_virtual_id(id: &str) -> bool {
    id.starts_with(VIRTUAL_PREFIX)
}

/// Reject user-space imports that attempt to use the `\0zenith:` namespace.
/// Returns `Err` if the specifier collides with internal virtual IDs.
/// This prevents namespace pollution and ensures virtual modules are hermetically sealed.
pub fn reject_external_zenith_import(specifier: &str) -> Result<(), BundleError> {
    // User specifiers should never start with \0 (null byte prefix)
    // Any specifier containing "zenith:" after a null byte is an internal ID
    if specifier.starts_with('\0') {
        return Err(BundleError::ValidationError(format!(
            "Cannot import internal virtual module '{}' — \\0zenith: namespace is reserved",
            specifier
        )));
    }
    // Also reject literal string "\0zenith:" in non-null-prefixed specifiers
    // (e.g. someone trying to escape the prefix)
    if specifier.contains("\\0zenith:") || specifier.contains("%00zenith:") {
        return Err(BundleError::ValidationError(format!(
            "Cannot reference internal virtual namespace in specifier '{}'",
            specifier
        )));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Rolldown Commit Pin
// ---------------------------------------------------------------------------

/// Expected Rolldown git commit. If the actual Rolldown version differs,
/// determinism guarantees may be invalidated.
pub const EXPECTED_ROLLDOWN_COMMIT: &str = "67a1f58";

// ---------------------------------------------------------------------------
// JS String Escaping
// ---------------------------------------------------------------------------

/// Escape a string for safe embedding inside a JS template literal (backtick string).
/// Prevents injection by escaping backticks, backslashes, and `${`.
pub fn escape_js_template_literal(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 16);
    let chars: Vec<char> = s.chars().collect();
    let len = chars.len();
    let mut i = 0;

    while i < len {
        match chars[i] {
            '\\' => {
                out.push_str("\\\\");
            }
            '`' => {
                out.push_str("\\`");
            }
            '$' if i + 1 < len && chars[i + 1] == '{' => {
                out.push_str("\\${");
                i += 1; // skip the '{'
            }
            c => {
                out.push(c);
            }
        }
        i += 1;
    }
    out
}

/// Escape a string for safe embedding inside a JS double-quoted string literal.
pub fn escape_js_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 16);
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c => out.push(c),
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Virtual Entry Generation
// ---------------------------------------------------------------------------

/// Generate the virtual entry module JS source for a compiled page.
///
/// The entry contains:
/// - `__zenith_html` — the HTML template string
/// - `__zenith_expr` — the expression table
/// - A default export function (hydration stub)
pub fn generate_virtual_entry(output: &CompilerOutput) -> String {
    let html_escaped = escape_js_template_literal(&output.html);

    let expr_items: Vec<String> = output
        .expressions
        .iter()
        .map(|e| format!("\"{}\"", escape_js_string(e)))
        .collect();

    let expr_array = expr_items.join(", ");

    format!(
        r#"export const __zenith_html = `{}`;
export const __zenith_expr = [{}];
export const __zenith_contract = "v0";
export default function __zenith_page() {{
  return {{ html: __zenith_html, expressions: __zenith_expr, contract: __zenith_contract }};
}}"#,
        html_escaped, expr_array
    )
}

// ---------------------------------------------------------------------------
// Canonicalize Page ID
// ---------------------------------------------------------------------------

/// Derive a deterministic page ID from a file path.
/// Strips extensions, normalizes separators, and lowercases.
pub fn canonicalize_page_id(page_path: &str) -> String {
    let path = std::path::Path::new(page_path);
    let stem = path.file_stem().unwrap_or_default().to_string_lossy();
    // Use the stem as the page ID, lowercased
    stem.to_lowercase()
}

// ---------------------------------------------------------------------------
// Post-Build Validation
// ---------------------------------------------------------------------------

/// Validate that the bundled output contains all expected `data-zx-e` placeholders.
pub fn validate_placeholders(html: &str, expression_count: usize) -> Result<(), Vec<Diagnostic>> {
    let mut found_indices = std::collections::HashSet::new();

    // Regex to find all data-zx-* attributes and capture their values (quoted or unquoted)
    // Matches: data-zx-something="value" OR data-zx-something='value' OR data-zx-something=value
    let re = Regex::new(r#"data-zx-[a-z-]+=(?:"([^"]+)"|'([^']+)'|([^\s>"']+))"#).unwrap();

    for cap in re.captures_iter(html) {
        // Value is in group 1, 2, or 3
        let val = cap
            .get(1)
            .or(cap.get(2))
            .or(cap.get(3))
            .map(|m| m.as_str())
            .unwrap_or("");

        // Parse space-separated indices
        for part in val.split_whitespace() {
            if let Ok(idx) = part.parse::<usize>() {
                found_indices.insert(idx);
            }
        }
    }

    let mut missing = Vec::new();
    for i in 0..expression_count {
        if !found_indices.contains(&i) {
            missing.push(Diagnostic {
                level: DiagnosticLevel::Error,
                message: format!("Missing placeholder for expression index {}", i),
                context: Some(format!(
                    "Expected index {} in a data-zx-e or data-zx-on-* attribute",
                    i
                )),
            });
        }
    }

    if missing.is_empty() {
        Ok(())
    } else {
        Err(missing)
    }
}

/// Validate that compiled expressions match metadata expressions exactly.
pub fn validate_expressions(compiled: &[String], metadata: &[String]) -> Result<(), BundleError> {
    if compiled.len() != metadata.len() {
        return Err(BundleError::ExpressionMismatch {
            expected: metadata.len(),
            got: compiled.len(),
        });
    }

    for (i, (got, expected)) in compiled.iter().zip(metadata.iter()).enumerate() {
        if got != expected {
            return Err(BundleError::ExpressionContentMismatch {
                index: i,
                expected: expected.clone(),
                got: got.clone(),
            });
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_virtual_entry_id() {
        assert_eq!(virtual_entry_id("home"), "\0zenith:entry:home");
    }

    #[test]
    fn test_virtual_css_id() {
        assert_eq!(virtual_css_id("home"), "\0zenith:css:home");
    }

    #[test]
    fn test_extract_page_id() {
        assert_eq!(extract_page_id("\0zenith:entry:home"), Some("home"));
        assert_eq!(extract_page_id("\0zenith:css:about"), Some("about"));
        assert_eq!(extract_page_id("other"), None);
    }

    #[test]
    fn test_is_zen_file() {
        assert!(is_zen_file("page.zen"));
        assert!(is_zen_file("/foo/bar.zen"));
        assert!(!is_zen_file("page.tsx"));
    }

    #[test]
    fn test_escape_js_template_literal() {
        assert_eq!(escape_js_template_literal("hello"), "hello");
        assert_eq!(escape_js_template_literal("a`b"), "a\\`b");
        assert_eq!(escape_js_template_literal("${x}"), "\\${x}");
        assert_eq!(escape_js_template_literal("a\\b"), "a\\\\b");
    }

    #[test]
    fn test_escape_js_string() {
        assert_eq!(escape_js_string(r#"he said "hi""#), r#"he said \"hi\""#);
        assert_eq!(escape_js_string("line1\nline2"), "line1\\nline2");
    }

    #[test]
    fn test_canonicalize_page_id() {
        assert_eq!(canonicalize_page_id("index.zen"), "index");
        assert_eq!(canonicalize_page_id("/pages/About.zen"), "about");
    }

    #[test]
    fn test_validate_expressions_match() {
        let compiled = vec!["a".into(), "b".into()];
        let metadata = vec!["a".into(), "b".into()];
        assert!(validate_expressions(&compiled, &metadata).is_ok());
    }

    #[test]
    fn test_validate_expressions_length_mismatch() {
        let compiled = vec!["a".into()];
        let metadata = vec!["a".into(), "b".into()];
        assert!(validate_expressions(&compiled, &metadata).is_err());
    }

    #[test]
    fn test_validate_expressions_content_mismatch() {
        let compiled = vec!["a".into(), "c".into()];
        let metadata = vec!["a".into(), "b".into()];
        let err = validate_expressions(&compiled, &metadata).unwrap_err();
        match err {
            BundleError::ExpressionContentMismatch { index, .. } => assert_eq!(index, 1),
            _ => panic!("Expected ExpressionContentMismatch"),
        }
    }

    #[test]
    fn test_generate_virtual_entry() {
        let output = CompilerOutput {
            ir_version: 1,
            html: "<div data-zx-e=\"0\"></div>".into(),
            expressions: vec!["title".into()],
            hoisted: Default::default(),
            components_scripts: Default::default(),
            component_instances: Default::default(),
            signals: Default::default(),
            expression_bindings: Default::default(),
            marker_bindings: Default::default(),
            event_bindings: Default::default(),
        };
        let entry = generate_virtual_entry(&output);
        assert!(entry.contains("__zenith_html"));
        assert!(entry.contains("__zenith_expr"));
        assert!(entry.contains("\"title\""));
        // Inside a JS template literal, double quotes are NOT escaped
        assert!(entry.contains("data-zx-e=\"0\""));
    }

    #[test]
    fn test_validate_placeholders_all_present() {
        let html = r#"<div data-zx-e="0"><span data-zx-e="1"></span></div>"#;
        assert!(validate_placeholders(html, 2).is_ok());
    }

    #[test]
    fn test_validate_placeholders_with_events() {
        let html = r#"<button data-zx-on-click="0"></button>"#;
        assert!(validate_placeholders(html, 1).is_ok());
    }

    #[test]
    fn test_validate_placeholders_missing() {
        let html = r#"<div data-zx-e="0"></div>"#;
        let result = validate_placeholders(html, 2);
        assert!(result.is_err());
        let diagnostics = result.unwrap_err();
        assert_eq!(diagnostics.len(), 1);
        assert!(diagnostics[0].message.contains("index 1"));
    }
}
