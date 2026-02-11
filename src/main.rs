use std::env;
use std::collections::BTreeMap;
use std::fs;
use std::io::{self, Read};
use std::path::PathBuf;
use std::process;

use regex::Regex;
use serde::{Deserialize, Serialize};
use zenith_bundler::CompilerOutput;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct BundlerInput {
    route: String,
    file: String,
    ir: CompilerIr,
    #[serde(default)]
    router: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct CompilerIr {
    ir_version: u32,
    html: String,
    expressions: Vec<String>,
    #[serde(default)]
    hoisted: CompilerHoisted,
    #[serde(default)]
    components_scripts: BTreeMap<String, CompilerComponentScript>,
    #[serde(default)]
    component_instances: Vec<CompilerComponentInstance>,
    #[serde(default)]
    signals: Vec<CompilerSignal>,
    #[serde(default)]
    expression_bindings: Vec<CompilerExpressionBinding>,
    #[serde(default)]
    marker_bindings: Vec<MarkerBinding>,
    #[serde(default)]
    event_bindings: Vec<EventBinding>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct CompilerHoisted {
    #[serde(default)]
    imports: Vec<String>,
    #[serde(default)]
    declarations: Vec<String>,
    #[serde(default)]
    functions: Vec<String>,
    #[serde(default)]
    signals: Vec<String>,
    #[serde(default)]
    state: Vec<CompilerStateBinding>,
    #[serde(default)]
    code: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct CompilerStateBinding {
    key: String,
    value: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct CompilerComponentScript {
    hoist_id: String,
    factory: String,
    #[serde(default)]
    imports: Vec<String>,
    code: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct CompilerComponentInstance {
    instance: String,
    hoist_id: String,
    selector: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CompilerSignal {
    id: usize,
    kind: String,
    state_index: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CompilerExpressionBinding {
    marker_index: usize,
    #[serde(default)]
    signal_index: Option<usize>,
    #[serde(default)]
    state_index: Option<usize>,
    #[serde(default)]
    component_instance: Option<String>,
    #[serde(default)]
    component_binding: Option<String>,
    #[serde(default)]
    literal: Option<String>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RouterManifest {
    routes: Vec<RouterRouteEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RouterRouteEntry {
    path: String,
    output: String,
    html: String,
    expressions: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum MarkerKind {
    Text,
    Attr,
    Event,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct MarkerBinding {
    index: usize,
    kind: MarkerKind,
    selector: String,
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    attr: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct EventBinding {
    index: usize,
    event: String,
    selector: String,
}

fn main() {
    if let Err(err) = run() {
        eprintln!("[zenith-bundler] {}", err);
        process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let out_dir = parse_out_dir()?;

    let mut stdin_payload = String::new();
    io::stdin()
        .read_to_string(&mut stdin_payload)
        .map_err(|e| format!("failed to read stdin: {e}"))?;

    if stdin_payload.trim().is_empty() {
        return Err("stdin payload is empty".into());
    }

    let payload: BundlerInput =
        serde_json::from_str(&stdin_payload).map_err(|e| format!("invalid input JSON: {e}"))?;
    validate_payload(&payload)?;

    let mut html = ensure_document_html(&payload.ir.html);

    fs::create_dir_all(&out_dir)
        .map_err(|e| format!("failed to create output dir '{}': {e}", out_dir.display()))?;

    let runtime_required =
        !payload.ir.expressions.is_empty() || !payload.ir.component_instances.is_empty();
    if runtime_required {
        let (markers, events) = if payload.ir.marker_bindings.is_empty() {
            derive_binding_tables(&payload.ir)?
        } else {
            (
                payload.ir.marker_bindings.clone(),
                payload.ir.event_bindings.clone(),
            )
        };
        let runtime_rel = ensure_runtime_asset(&out_dir)?;
        let runtime_script_src = format!("/{runtime_rel}");
        let runtime_import_spec = runtime_import_specifier(&runtime_rel)?;
        let component_assets = emit_component_assets(
            &out_dir,
            &payload.ir.components_scripts,
            &runtime_import_spec,
        )?;
        let js = generate_entry_js(
            &payload.ir,
            &runtime_import_spec,
            &markers,
            &events,
            &component_assets,
        )?;
        let js_hash = stable_hash_8(&js);
        let js_rel = format!("assets/{js_hash}.js");
        let js_path = out_dir.join(&js_rel);
        if let Some(parent) = js_path.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| format!("failed to create asset dir '{}': {e}", parent.display()))?;
        }
        fs::write(&js_path, js)
            .map_err(|e| format!("failed to write asset '{}': {e}", js_path.display()))?;

        html = inject_script_once(&html, &runtime_script_src, "data-zx-runtime");
        html = inject_script_once(&html, &format!("/{js_rel}"), "data-zx-page");
    }

    if payload.router {
        let output_path = route_to_output_path(&payload.route)
            .to_string_lossy()
            .replace('\\', "/");

        upsert_router_manifest(
            &out_dir,
            RouterRouteEntry {
                path: payload.route.clone(),
                output: output_path,
                html: payload.ir.html.clone(),
                expressions: payload.ir.expressions.clone(),
            },
        )?;

        let router_js = generate_router_runtime_js();
        let router_hash = stable_hash_8(&router_js);
        let router_rel = format!("assets/router.{router_hash}.js");
        let router_path = out_dir.join(&router_rel);
        if let Some(parent) = router_path.parent() {
            fs::create_dir_all(parent).map_err(|e| {
                format!(
                    "failed to create router asset dir '{}': {e}",
                    parent.display()
                )
            })?;
        }
        fs::write(&router_path, router_js).map_err(|e| {
            format!(
                "failed to write router asset '{}': {e}",
                router_path.display()
            )
        })?;

        html = inject_script_once(&html, &format!("/{router_rel}"), "data-zx-router");
    }

    let html_rel = route_to_output_path(&payload.route);
    let html_path = out_dir.join(html_rel);
    if let Some(parent) = html_path.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("failed to create html dir '{}': {e}", parent.display()))?;
    }
    fs::write(&html_path, html)
        .map_err(|e| format!("failed to write html '{}': {e}", html_path.display()))?;

    Ok(())
}

fn parse_out_dir() -> Result<PathBuf, String> {
    let mut out_dir: Option<PathBuf> = None;
    let mut args = env::args().skip(1);

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--out-dir" => {
                let value = args
                    .next()
                    .ok_or_else(|| "missing value for --out-dir".to_string())?;
                out_dir = Some(PathBuf::from(value));
            }
            _ => {
                return Err(format!(
                    "unknown argument '{arg}'. usage: zenith-bundler --out-dir <path>"
                ));
            }
        }
    }

    out_dir.ok_or_else(|| "required flag missing: --out-dir <path>".to_string())
}

fn validate_payload(payload: &BundlerInput) -> Result<(), String> {
    if payload.ir.ir_version != 1 {
        return Err(format!(
            "unsupported input.ir.ir_version {} (expected 1)",
            payload.ir.ir_version
        ));
    }
    if payload.route.trim().is_empty() {
        return Err("input.route must be a non-empty string".into());
    }
    if !payload.route.starts_with('/') {
        return Err("input.route must start with '/'".into());
    }
    if payload.file.trim().is_empty() {
        return Err("input.file must be a non-empty string".into());
    }
    if payload.ir.html.trim().is_empty() {
        return Err("input.ir.html must be a non-empty string".into());
    }
    if !payload.ir.expression_bindings.is_empty()
        && payload.ir.expression_bindings.len() != payload.ir.expressions.len()
    {
        return Err(format!(
            "input.ir.expression_bindings length ({}) must match input.ir.expressions length ({})",
            payload.ir.expression_bindings.len(),
            payload.ir.expressions.len()
        ));
    }
    if !payload.ir.marker_bindings.is_empty()
        && payload.ir.marker_bindings.len() != payload.ir.expressions.len()
    {
        return Err(format!(
            "input.ir.marker_bindings length ({}) must match input.ir.expressions length ({})",
            payload.ir.marker_bindings.len(),
            payload.ir.expressions.len()
        ));
    }
    for signal in &payload.ir.signals {
        if signal.kind != "signal" {
            return Err(format!(
                "input.ir.signals[].kind must be 'signal', got '{}'",
                signal.kind
            ));
        }
        if signal.state_index >= payload.ir.hoisted.state.len() {
            return Err(format!(
                "input.ir.signals[{}].state_index out of bounds: {}",
                signal.id, signal.state_index
            ));
        }
    }
    for (position, binding) in payload.ir.expression_bindings.iter().enumerate() {
        if binding.marker_index >= payload.ir.expressions.len() {
            return Err(format!(
                "input.ir.expression_bindings[{position}].marker_index out of bounds: {}",
                binding.marker_index
            ));
        }
        if let Some(state_index) = binding.state_index {
            if state_index >= payload.ir.hoisted.state.len() {
                return Err(format!(
                    "input.ir.expression_bindings[{position}].state_index out of bounds: {}",
                    state_index
                ));
            }
        }
        if let Some(signal_index) = binding.signal_index {
            if signal_index >= payload.ir.signals.len() {
                return Err(format!(
                    "input.ir.expression_bindings[{position}].signal_index out of bounds: {}",
                    signal_index
                ));
            }
        }
    }
    for (hoist_id, script) in &payload.ir.components_scripts {
        if hoist_id.trim().is_empty() {
            return Err("input.ir.components_scripts contains an empty hoist_id key".into());
        }
        if script.code.trim().is_empty() {
            return Err(format!(
                "input.ir.components_scripts['{}'].code must be non-empty",
                hoist_id
            ));
        }
        if script.factory.trim().is_empty() {
            return Err(format!(
                "input.ir.components_scripts['{}'].factory must be non-empty",
                hoist_id
            ));
        }
        if script.hoist_id != *hoist_id {
            return Err(format!(
                "input.ir.components_scripts key '{}' mismatches hoist_id '{}'",
                hoist_id, script.hoist_id
            ));
        }
    }
    for instance in &payload.ir.component_instances {
        if instance.instance.trim().is_empty() {
            return Err("input.ir.component_instances[].instance must be non-empty".into());
        }
        if instance.selector.trim().is_empty() {
            return Err("input.ir.component_instances[].selector must be non-empty".into());
        }
        if !payload
            .ir
            .components_scripts
            .contains_key(&instance.hoist_id)
        {
            return Err(format!(
                "input.ir.component_instances references unknown hoist_id '{}'",
                instance.hoist_id
            ));
        }
    }

    if !payload.ir.marker_bindings.is_empty() {
        let mut seen = BTreeMap::new();
        for marker in &payload.ir.marker_bindings {
            if marker.index >= payload.ir.expressions.len() {
                return Err(format!(
                    "input.ir.marker_bindings index out of bounds: {}",
                    marker.index
                ));
            }
            if seen.insert(marker.index, true).is_some() {
                return Err(format!(
                    "input.ir.marker_bindings contains duplicate index {}",
                    marker.index
                ));
            }
        }
    }

    Ok(())
}

fn ensure_document_html(fragment_or_doc: &str) -> String {
    if fragment_or_doc.contains("<html") {
        return fragment_or_doc.to_string();
    }
    format!(
        "<!DOCTYPE html><html><head></head><body>{}</body></html>",
        fragment_or_doc
    )
}

fn inject_script_once(html: &str, script_src: &str, marker_attr: &str) -> String {
    if html.contains(script_src) {
        return html.to_string();
    }
    let script_tag =
        format!("<script type=\"module\" src=\"{script_src}\" {marker_attr}></script>");
    if html.contains("</body>") {
        return html.replacen("</body>", &format!("{script_tag}</body>"), 1);
    }
    format!("{html}{script_tag}")
}

fn route_to_output_path(route_path: &str) -> PathBuf {
    if route_path == "/" {
        return PathBuf::from("index.html");
    }

    let mut out = PathBuf::new();
    for segment in route_path.split('/').filter(|s| !s.is_empty()) {
        if segment.starts_with(':') {
            // Dynamic segments are rewritten by preview/router to this static shell.
            // Example: /users/:id -> dist/users/index.html
            continue;
        }
        out.push(segment);
    }
    out.push("index.html");
    out
}

fn stable_hash_8(content: &str) -> String {
    let mut hash: i32 = 0;
    for byte in content.bytes() {
        hash = hash
            .wrapping_shl(5)
            .wrapping_sub(hash)
            .wrapping_add(byte as i32);
    }
    let normalized = hash.wrapping_abs() as u32;
    format!("{normalized:08x}")
}

fn derive_binding_tables(ir: &CompilerIr) -> Result<(Vec<MarkerBinding>, Vec<EventBinding>), String> {
    let expression_count = ir.expressions.len();
    if expression_count == 0 {
        return Ok((Vec::new(), Vec::new()));
    }

    let mut marker_slots: Vec<Option<MarkerBinding>> = vec![None; expression_count];
    let mut event_bindings = Vec::new();

    let attr_re = Regex::new(r#"data-zx-([A-Za-z0-9_-]+)=(?:"([^"]+)"|'([^']+)'|([^\s>"']+))"#)
        .map_err(|e| format!("failed to compile binding regex: {e}"))?;

    for captures in attr_re.captures_iter(&ir.html) {
        let attr_name = captures
            .get(1)
            .map(|m| m.as_str())
            .ok_or_else(|| "failed to parse data-zx attribute name".to_string())?;
        let raw_value = captures
            .get(2)
            .or_else(|| captures.get(3))
            .or_else(|| captures.get(4))
            .map(|m| m.as_str())
            .unwrap_or("");

        if attr_name == "e" {
            for part in raw_value.split_whitespace() {
                let index = parse_expression_index(part, expression_count, "data-zx-e")?;
                insert_marker(
                    &mut marker_slots,
                    MarkerBinding {
                        index,
                        kind: MarkerKind::Text,
                        selector: format!(r#"[data-zx-e~="{index}"]"#),
                        attr: None,
                    },
                )?;
            }
            continue;
        }

        if attr_name == "c" {
            continue;
        }

        if let Some(event_name) = attr_name.strip_prefix("on-") {
            let index = parse_expression_index(raw_value, expression_count, "data-zx-on-*")?;
            let selector = format!(r#"[data-zx-on-{event_name}="{index}"]"#);
            insert_marker(
                &mut marker_slots,
                MarkerBinding {
                    index,
                    kind: MarkerKind::Event,
                    selector: selector.clone(),
                    attr: None,
                },
            )?;
            event_bindings.push(EventBinding {
                index,
                event: event_name.to_string(),
                selector,
            });
            continue;
        }

        let index = parse_expression_index(raw_value, expression_count, "data-zx-*")?;
        insert_marker(
            &mut marker_slots,
            MarkerBinding {
                index,
                kind: MarkerKind::Attr,
                selector: format!(r#"[data-zx-{attr_name}="{index}"]"#),
                attr: Some(attr_name.to_string()),
            },
        )?;
    }

    let mut markers = Vec::with_capacity(expression_count);
    for (index, marker) in marker_slots.into_iter().enumerate() {
        if let Some(binding) = marker {
            markers.push(binding);
            continue;
        }
        return Err(format!(
            "marker/expression mismatch: missing marker for expression index {index}"
        ));
    }

    Ok((markers, event_bindings))
}

fn parse_expression_index(raw: &str, expression_count: usize, context: &str) -> Result<usize, String> {
    let parsed = raw
        .parse::<usize>()
        .map_err(|_| format!("invalid expression index '{raw}' in {context}"))?;

    if parsed >= expression_count {
        return Err(format!(
            "out-of-bounds expression index {parsed} in {context}; expression count is {expression_count}"
        ));
    }

    Ok(parsed)
}

fn insert_marker(slots: &mut [Option<MarkerBinding>], marker: MarkerBinding) -> Result<(), String> {
    let index = marker.index;

    if index >= slots.len() {
        return Err(format!(
            "marker index {} out of bounds; marker slots length is {}",
            index,
            slots.len()
        ));
    }

    if slots[index].is_some() {
        return Err(format!(
            "duplicate marker index {} detected while deriving binding tables",
            index
        ));
    }

    slots[index] = Some(marker);
    Ok(())
}

fn runtime_import_specifier(runtime_rel: &str) -> Result<String, String> {
    let runtime_path = PathBuf::from(runtime_rel);
    let file_name = runtime_path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| format!("invalid runtime asset path '{runtime_rel}'"))?;
    Ok(format!("./{file_name}"))
}

fn ensure_runtime_asset(out_dir: &PathBuf) -> Result<String, String> {
    let runtime_js = generate_runtime_module_js();
    let runtime_hash = stable_hash_8(&runtime_js);
    let runtime_rel = format!("assets/runtime.{runtime_hash}.js");
    let runtime_path = out_dir.join(&runtime_rel);

    if !runtime_path.exists() {
        if let Some(parent) = runtime_path.parent() {
            fs::create_dir_all(parent).map_err(|e| {
                format!(
                    "failed to create runtime asset dir '{}': {e}",
                    parent.display()
                )
            })?;
        }
        fs::write(&runtime_path, runtime_js).map_err(|e| {
            format!(
                "failed to write runtime asset '{}': {e}",
                runtime_path.display()
            )
        })?;
    }

    Ok(runtime_rel)
}

fn emit_component_assets(
    out_dir: &PathBuf,
    components: &BTreeMap<String, CompilerComponentScript>,
    runtime_import_spec: &str,
) -> Result<BTreeMap<String, String>, String> {
    let mut out = BTreeMap::new();
    for (hoist_id, component) in components {
        let mut module_source = String::new();
        for import_line in &component.imports {
            module_source.push_str(import_line.trim());
            module_source.push('\n');
        }
        module_source.push_str(&format!(
            "import {{ signal, state, zeneffect }} from '{}';\n",
            runtime_import_spec
        ));
        module_source.push_str(&format!(
            "const __zenith_runtime = Object.freeze({{ signal, state, zeneffect }});\n"
        ));

        module_source.push_str(&component.code);
        module_source.push('\n');

        let module_hash = stable_hash_8(&module_source);
        let rel = format!("assets/component.{}.{}.js", sanitize_asset_token(hoist_id), module_hash);
        let path = out_dir.join(&rel);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| {
                format!(
                    "failed to create component asset dir '{}': {e}",
                    parent.display()
                )
            })?;
        }
        fs::write(&path, module_source).map_err(|e| {
            format!(
                "failed to write component asset '{}': {e}",
                path.display()
            )
        })?;

        out.insert(hoist_id.clone(), rel);
    }
    Ok(out)
}

fn sanitize_asset_token(input: &str) -> String {
    input
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' { ch } else { '_' })
        .collect()
}

fn generate_entry_js(
    ir: &CompilerIr,
    runtime_import_spec: &str,
    markers: &[MarkerBinding],
    events: &[EventBinding],
    component_assets: &BTreeMap<String, String>,
) -> Result<String, String> {
    let compiler_output = CompilerOutput {
        ir_version: ir.ir_version,
        html: ir.html.clone(),
        expressions: ir.expressions.clone(),
        hoisted: Default::default(),
        components_scripts: Default::default(),
        component_instances: Default::default(),
        signals: Default::default(),
        expression_bindings: Default::default(),
        marker_bindings: Default::default(),
        event_bindings: Default::default(),
    };

    let markers_json = serde_json::to_string(markers)
        .map_err(|e| format!("failed to serialize marker table: {e}"))?;
    let events_json = serde_json::to_string(events)
        .map_err(|e| format!("failed to serialize event table: {e}"))?;

    let mut js = zenith_bundler::utils::generate_virtual_entry(&compiler_output);
    for block in &ir.hoisted.code {
        let trimmed = block.trim();
        if !trimmed.is_empty() {
            js.push('\n');
            js.push_str(trimmed);
            js.push('\n');
        }
    }
    js.push_str(&format!(
        "\nconst __zenith_markers = {};\n",
        markers_json
    ));
    js.push_str(&format!(
        "const __zenith_events = {};\n",
        events_json
    ));
    let signals_json = serde_json::to_string(&ir.signals)
        .map_err(|e| format!("failed to serialize signal table: {e}"))?;
    let expression_bindings_json = if ir.expression_bindings.is_empty() {
        fallback_expression_bindings(ir)?
    } else {
        serde_json::to_string(&ir.expression_bindings)
            .map_err(|e| format!("failed to serialize expression table: {e}"))?
    };

    js.push_str(&generate_state_table_js(&ir.hoisted.state)?);
    js.push_str(&format!(
        "const __zenith_ir_version = {};\n",
        ir.ir_version
    ));
    js.push_str(&format!(
        "const __zenith_signals = Object.freeze({});\n",
        signals_json
    ));
    js.push_str(&format!(
        "const __zenith_expression_bindings = Object.freeze({});\n",
        expression_bindings_json
    ));
    let (component_imports, components_table) =
        generate_component_bootstrap_js(ir, component_assets)?;
    if !component_imports.is_empty() {
        js.push_str(&component_imports);
    }
    js.push_str(&format!(
        "import {{ hydrate, signal, state, zeneffect }} from '{}';\n",
        runtime_import_spec
    ));
    js.push_str(&format!("const __zenith_components = {};\n", components_table));
    js.push_str("hydrate({\n");
    js.push_str("  root: document,\n");
    js.push_str("  ir_version: __zenith_ir_version,\n");
    js.push_str("  expressions: __zenith_expression_bindings,\n");
    js.push_str("  markers: __zenith_markers,\n");
    js.push_str("  events: __zenith_events,\n");
    js.push_str("  state_values: __zenith_state_values,\n");
    js.push_str("  signals: __zenith_signals,\n");
    js.push_str("  components: __zenith_components\n");
    js.push_str("});\n");

    Ok(js)
}

fn generate_state_table_js(bindings: &[CompilerStateBinding]) -> Result<String, String> {
    if bindings.is_empty() {
        return Ok("const __zenith_state_values = Object.freeze([]);\n".to_string());
    }

    let mut out = String::from("const __zenith_state_values = Object.freeze([\n");
    for binding in bindings {
        out.push_str("  ");
        out.push_str(binding.value.trim());
        out.push_str(",\n");
    }
    out.push_str("]);\n");
    Ok(out)
}

fn fallback_expression_bindings(ir: &CompilerIr) -> Result<String, String> {
    let bindings: Vec<CompilerExpressionBinding> = ir
        .expressions
        .iter()
        .enumerate()
        .map(|(index, value)| CompilerExpressionBinding {
            marker_index: index,
            signal_index: None,
            state_index: None,
            component_instance: None,
            component_binding: None,
            literal: Some(value.clone()),
        })
        .collect();
    serde_json::to_string(&bindings)
        .map_err(|e| format!("failed to serialize fallback expressions: {e}"))
}

fn generate_component_bootstrap_js(
    ir: &CompilerIr,
    component_assets: &BTreeMap<String, String>,
) -> Result<(String, String), String> {
    if ir.component_instances.is_empty() {
        return Ok((String::new(), "[]".to_string()));
    }

    let mut aliases = BTreeMap::new();
    let mut imports = String::new();
    for (hoist_id, rel) in component_assets {
        let alias = format!("__zenith_component_{}", sanitize_asset_token(hoist_id));
        let component_path = PathBuf::from(rel);
        let file_name = component_path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| format!("invalid component asset path '{rel}'"))?;
        imports.push_str(&format!("import {} from './{}';\n", alias, file_name));
        aliases.insert(hoist_id.clone(), alias);
    }

    let mut components = String::from("[");
    for (index, instance) in ir.component_instances.iter().enumerate() {
        let create_alias = aliases.get(&instance.hoist_id).ok_or_else(|| {
            format!(
                "missing component asset mapping for hoist_id '{}'",
                instance.hoist_id
            )
        })?;
        if index > 0 {
            components.push(',');
        }
        let instance_json = serde_json::to_string(&instance.instance)
            .map_err(|e| format!("failed to serialize component instance id: {e}"))?;
        let selector_json = serde_json::to_string(&instance.selector)
            .map_err(|e| format!("failed to serialize component selector: {e}"))?;
        let hoist_json = serde_json::to_string(&instance.hoist_id)
            .map_err(|e| format!("failed to serialize component hoist id: {e}"))?;
        components.push_str(&format!(
            "{{instance:{instance_json},selector:{selector_json},hoist_id:{hoist_json},create:{create_alias}}}"
        ));
    }
    components.push(']');

    Ok((imports, components))
}

fn generate_runtime_module_js() -> String {
    r#"const BOOLEAN_ATTRIBUTES = new Set(['disabled', 'checked', 'readonly', 'required', 'selected', 'open', 'hidden']);
const __listeners = [];
const __components = [];

function cleanup() {
  for (let i = 0; i < __components.length; i++) {
    const instance = __components[i];
    if (instance && typeof instance.destroy === 'function') {
      instance.destroy();
    }
  }
  __components.length = 0;

  for (let i = 0; i < __listeners.length; i++) {
    const item = __listeners[i];
    item.node.removeEventListener(item.event, item.handler);
  }
  __listeners.length = 0;
}

function __coerceText(value) {
  if (value === null || value === undefined || value === false) return '';
  return String(value);
}

function __applyAttribute(node, attrName, value) {
  if (attrName === 'class' || attrName === 'className') {
    node.className = value === null || value === undefined || value === false ? '' : String(value);
    return;
  }

  if (attrName === 'style') {
    if (value === null || value === undefined || value === false) {
      node.removeAttribute('style');
      return;
    }
    if (typeof value === 'string') {
      node.setAttribute('style', value);
      return;
    }
    if (typeof value === 'object') {
      const entries = Object.entries(value);
      let styleText = '';
      for (let i = 0; i < entries.length; i++) {
        styleText += entries[i][0] + ': ' + entries[i][1] + ';';
      }
      node.setAttribute('style', styleText);
      return;
    }
    node.setAttribute('style', String(value));
    return;
  }

  if (BOOLEAN_ATTRIBUTES.has(attrName)) {
    if (value) {
      node.setAttribute(attrName, '');
    } else {
      node.removeAttribute(attrName);
    }
    return;
  }

  if (value === null || value === undefined || value === false) {
    node.removeAttribute(attrName);
    return;
  }

  node.setAttribute(attrName, String(value));
}

function __getComponentBinding(bindingsByInstance, instance, binding) {
  if (!bindingsByInstance || typeof bindingsByInstance !== 'object') return undefined;
  const instanceBindings = bindingsByInstance[instance];
  if (!instanceBindings || typeof instanceBindings !== 'object') return undefined;
  return instanceBindings[binding];
}

function __evaluateExpression(binding, stateValues, signalMap, componentBindings, mode) {
  if (!binding || typeof binding !== 'object') {
    throw new Error('[Zenith Runtime] expression binding must be an object');
  }

  if (!Number.isInteger(binding.marker_index) || binding.marker_index < 0) {
    throw new Error('[Zenith Runtime] expression binding requires marker_index');
  }

  if (binding.signal_index !== null && binding.signal_index !== undefined) {
    if (!Number.isInteger(binding.signal_index)) {
      throw new Error('[Zenith Runtime] expression.signal_index must be an integer');
    }
    const signalValue = signalMap.get(binding.signal_index);
    if (!signalValue || typeof signalValue.get !== 'function') {
      throw new Error('[Zenith Runtime] expression.signal_index did not resolve to a signal');
    }
    return mode === 'event' ? signalValue : signalValue.get();
  }

  if (binding.state_index !== null && binding.state_index !== undefined) {
    if (!Number.isInteger(binding.state_index) || binding.state_index < 0 || binding.state_index >= stateValues.length) {
      throw new Error('[Zenith Runtime] expression.state_index out of bounds');
    }
    const resolved = stateValues[binding.state_index];
    if (mode !== 'event' && typeof resolved === 'function') {
      return resolved();
    }
    return resolved;
  }

  if (typeof binding.component_instance === 'string' && typeof binding.component_binding === 'string') {
    const resolved = __getComponentBinding(componentBindings, binding.component_instance, binding.component_binding);
    if (mode !== 'event' && typeof resolved === 'function') {
      return resolved();
    }
    return resolved;
  }

  if (binding.literal !== null && binding.literal !== undefined) {
    return binding.literal;
  }

  return '';
}

function __resolveNodes(root, selector, index, kind) {
  const nodes = root.querySelectorAll(selector);
  if (!nodes || nodes.length === 0) {
    throw new Error('[Zenith Runtime] unresolved ' + kind + ' marker index ' + index + ' for selector "' + selector + '"');
  }
  return nodes;
}

export function hydrate(payload) {
  cleanup();

  if (!payload || typeof payload !== 'object') {
    throw new Error('[Zenith Runtime] hydrate(payload) requires an object payload');
  }
  if (payload.ir_version !== 1) {
    throw new Error('[Zenith Runtime] unsupported ir_version (expected 1)');
  }
  if (!payload.root || typeof payload.root.querySelectorAll !== 'function') {
    throw new Error('[Zenith Runtime] hydrate(payload) requires payload.root with querySelectorAll');
  }
  if (!Array.isArray(payload.expressions)) {
    throw new Error('[Zenith Runtime] hydrate(payload) requires expressions[]');
  }
  if (!Array.isArray(payload.markers)) {
    throw new Error('[Zenith Runtime] hydrate(payload) requires markers[]');
  }
  if (!Array.isArray(payload.events)) {
    throw new Error('[Zenith Runtime] hydrate(payload) requires events[]');
  }
  if (!Array.isArray(payload.state_values)) {
    throw new Error('[Zenith Runtime] hydrate(payload) requires state_values[]');
  }
  if (!Array.isArray(payload.signals)) {
    throw new Error('[Zenith Runtime] hydrate(payload) requires signals[]');
  }
  if (payload.components !== undefined && !Array.isArray(payload.components)) {
    throw new Error('[Zenith Runtime] hydrate(payload) requires components[] when provided');
  }
  if (payload.markers.length !== payload.expressions.length) {
    throw new Error('[Zenith Runtime] marker/expression mismatch: markers=' + payload.markers.length + ', expressions=' + payload.expressions.length);
  }

  const root = payload.root;
  const expressions = payload.expressions;
  const markers = payload.markers;
  const events = payload.events;
  const stateValues = payload.state_values;
  const signals = payload.signals;
  const components = Array.isArray(payload.components) ? payload.components : [];
  const componentBindings = Object.create(null);
  const signalMap = new Map();

  const runtimeApi = Object.freeze({ signal, state, zeneffect });
  for (let i = 0; i < components.length; i++) {
    const component = components[i];
    if (!component || typeof component !== 'object') {
      throw new Error('[Zenith Runtime] component at position ' + i + ' must be an object');
    }
    if (typeof component.selector !== 'string' || component.selector.length === 0) {
      throw new Error('[Zenith Runtime] component at position ' + i + ' requires selector');
    }
    if (typeof component.instance !== 'string' || component.instance.length === 0) {
      throw new Error('[Zenith Runtime] component at position ' + i + ' requires instance');
    }
    if (typeof component.create !== 'function') {
      throw new Error('[Zenith Runtime] component at position ' + i + ' requires create() function');
    }

    const hosts = __resolveNodes(root, component.selector, i, 'component');
    for (let j = 0; j < hosts.length; j++) {
      const instance = component.create(hosts[j], Object.freeze({}), runtimeApi);
      if (!instance || typeof instance !== 'object') {
        throw new Error('[Zenith Runtime] component factory for ' + component.instance + ' must return an object');
      }
      if (typeof instance.mount === 'function') {
        instance.mount();
      }
      if (typeof instance.destroy === 'function') {
        __components.push({ destroy: instance.destroy.bind(instance) });
      }
      if (instance.bindings && typeof instance.bindings === 'object') {
        componentBindings[component.instance] = instance.bindings;
      }
    }
  }

  const signalIds = new Set();
  for (let i = 0; i < signals.length; i++) {
    const entry = signals[i];
    if (!entry || typeof entry !== 'object') {
      throw new Error('[Zenith Runtime] signal descriptor at position ' + i + ' must be an object');
    }
    if (entry.kind !== 'signal') {
      throw new Error('[Zenith Runtime] signal descriptor at position ' + i + ' requires kind=\"signal\"');
    }
    if (!Number.isInteger(entry.id) || entry.id < 0) {
      throw new Error('[Zenith Runtime] signal descriptor at position ' + i + ' requires non-negative id');
    }
    if (signalIds.has(entry.id)) {
      throw new Error('[Zenith Runtime] duplicate signal id ' + entry.id);
    }
    signalIds.add(entry.id);
    if (!Number.isInteger(entry.state_index) || entry.state_index < 0 || entry.state_index >= stateValues.length) {
      throw new Error('[Zenith Runtime] signal descriptor at position ' + i + ' has out-of-bounds state_index');
    }

    const candidate = stateValues[entry.state_index];
    if (!candidate || typeof candidate !== 'object' || typeof candidate.get !== 'function' || typeof candidate.subscribe !== 'function') {
      throw new Error('[Zenith Runtime] signal descriptor id ' + entry.id + ' did not resolve to a signal object');
    }
    signalMap.set(entry.id, candidate);
  }

  const expressionMarkerIndices = new Set();
  for (let i = 0; i < expressions.length; i++) {
    const expression = expressions[i];
    if (!expression || typeof expression !== 'object') {
      throw new Error('[Zenith Runtime] expression at position ' + i + ' must be an object');
    }
    if (!Number.isInteger(expression.marker_index) || expression.marker_index < 0 || expression.marker_index >= expressions.length) {
      throw new Error('[Zenith Runtime] expression at position ' + i + ' has invalid marker_index');
    }
    if (expression.marker_index !== i) {
      throw new Error('[Zenith Runtime] expression table out of order at position ' + i + ': marker_index=' + expression.marker_index);
    }
    if (expressionMarkerIndices.has(expression.marker_index)) {
      throw new Error('[Zenith Runtime] duplicate expression marker_index ' + expression.marker_index);
    }
    expressionMarkerIndices.add(expression.marker_index);
  }

  const markerIndices = new Set();
  const markerByIndex = new Map();
  const markerNodesByIndex = new Map();
  for (let i = 0; i < markers.length; i++) {
    const marker = markers[i];
    if (!marker || typeof marker !== 'object') {
      throw new Error('[Zenith Runtime] marker at position ' + i + ' must be an object');
    }
    if (!Number.isInteger(marker.index) || marker.index < 0 || marker.index >= expressions.length) {
      throw new Error('[Zenith Runtime] marker at position ' + i + ' has out-of-bounds index');
    }
    if (marker.index !== i) {
      throw new Error('[Zenith Runtime] marker table out of order at position ' + i + ': index=' + marker.index);
    }
    if (markerIndices.has(marker.index)) {
      throw new Error('[Zenith Runtime] duplicate marker index ' + marker.index);
    }
    markerIndices.add(marker.index);
    markerByIndex.set(marker.index, marker);

    if (marker.kind === 'event') {
      continue;
    }

    if (typeof marker.selector !== 'string' || marker.selector.length === 0) {
      throw new Error('[Zenith Runtime] marker at position ' + i + ' requires selector');
    }

    const nodes = __resolveNodes(root, marker.selector, marker.index, marker.kind);
    markerNodesByIndex.set(marker.index, nodes);
    const value = __evaluateExpression(expressions[marker.index], stateValues, signalMap, componentBindings, marker.kind);

    for (let j = 0; j < nodes.length; j++) {
      if (marker.kind === 'text') {
        nodes[j].textContent = __coerceText(value);
      } else if (marker.kind === 'attr') {
        if (typeof marker.attr !== 'string' || marker.attr.length === 0) {
          throw new Error('[Zenith Runtime] attr marker at position ' + i + ' requires attr');
        }
        __applyAttribute(nodes[j], marker.attr, value);
      } else {
        throw new Error('[Zenith Runtime] marker at position ' + i + ' has invalid kind');
      }
    }
  }

  for (let i = 0; i < expressions.length; i++) {
    if (!markerIndices.has(i)) {
      throw new Error('[Zenith Runtime] missing marker index ' + i);
    }
  }

  function renderMarkerByIndex(index) {
    const marker = markerByIndex.get(index);
    if (!marker || marker.kind === 'event') return;
    const nodes = markerNodesByIndex.get(index) || __resolveNodes(root, marker.selector, marker.index, marker.kind);
    markerNodesByIndex.set(index, nodes);

    const value = __evaluateExpression(expressions[index], stateValues, signalMap, componentBindings, marker.kind);
    for (let j = 0; j < nodes.length; j++) {
      if (marker.kind === 'text') {
        nodes[j].textContent = __coerceText(value);
      } else if (marker.kind === 'attr') {
        __applyAttribute(nodes[j], marker.attr, value);
      }
    }
  }

  const dependentMarkersBySignal = new Map();
  for (let i = 0; i < expressions.length; i++) {
    const binding = expressions[i];
    if (!binding || typeof binding !== 'object') continue;
    if (!Number.isInteger(binding.signal_index)) continue;
    if (!dependentMarkersBySignal.has(binding.signal_index)) {
      dependentMarkersBySignal.set(binding.signal_index, []);
    }
    dependentMarkersBySignal.get(binding.signal_index).push(binding.marker_index);
  }

  for (const [signalId, markerIndicesForSignal] of dependentMarkersBySignal.entries()) {
    const targetSignal = signalMap.get(signalId);
    if (!targetSignal) {
      throw new Error('[Zenith Runtime] expression references unknown signal id ' + signalId);
    }
    const unsubscribe = targetSignal.subscribe(() => {
      for (let i = 0; i < markerIndicesForSignal.length; i++) {
        renderMarkerByIndex(markerIndicesForSignal[i]);
      }
    });
    if (typeof unsubscribe === 'function') {
      __components.push({ destroy: unsubscribe });
    }
  }

  const eventIndices = new Set();
  for (let i = 0; i < events.length; i++) {
    const binding = events[i];
    if (!binding || typeof binding !== 'object') {
      throw new Error('[Zenith Runtime] event binding at position ' + i + ' must be an object');
    }
    if (!Number.isInteger(binding.index) || binding.index < 0 || binding.index >= expressions.length) {
      throw new Error('[Zenith Runtime] event binding at position ' + i + ' has out-of-bounds index');
    }
    if (eventIndices.has(binding.index)) {
      throw new Error('[Zenith Runtime] duplicate event index ' + binding.index);
    }
    eventIndices.add(binding.index);
    if (typeof binding.event !== 'string' || binding.event.length === 0) {
      throw new Error('[Zenith Runtime] event binding at position ' + i + ' requires event name');
    }
    if (typeof binding.selector !== 'string' || binding.selector.length === 0) {
      throw new Error('[Zenith Runtime] event binding at position ' + i + ' requires selector');
    }

    const nodes = __resolveNodes(root, binding.selector, binding.index, 'event');
    const handler = __evaluateExpression(expressions[binding.index], stateValues, signalMap, componentBindings, 'event');
    if (typeof handler !== 'function') {
      throw new Error('[Zenith Runtime] event binding at index ' + binding.index + ' did not resolve to a function');
    }

    for (let j = 0; j < nodes.length; j++) {
      nodes[j].addEventListener(binding.event, handler);
      __listeners.push({ node: nodes[j], event: binding.event, handler });
    }
  }

  return cleanup;
}

export function signal(initialValue) {
  let value = initialValue;
  const subscribers = new Set();
  return {
    get() { return value; },
    set(nextValue) {
      if (Object.is(value, nextValue)) return value;
      value = nextValue;
      const snapshot = [...subscribers];
      for (let i = 0; i < snapshot.length; i++) snapshot[i](value);
      return value;
    },
    subscribe(fn) {
      if (typeof fn !== 'function') {
        throw new Error('[Zenith Runtime] signal.subscribe(fn) requires a function');
      }
      subscribers.add(fn);
      return function unsubscribe() { subscribers.delete(fn); };
    }
  };
}

export function state(initialValue) {
  if (!initialValue || typeof initialValue !== 'object' || Array.isArray(initialValue)) {
    throw new Error('[Zenith Runtime] state(initial) requires a plain object');
  }
  let current = Object.freeze({ ...initialValue });
  const subscribers = new Set();
  return {
    get() { return current; },
    set(nextPatch) {
      const nextValue = typeof nextPatch === 'function'
        ? nextPatch(current)
        : { ...current, ...nextPatch };
      if (!nextValue || typeof nextValue !== 'object' || Array.isArray(nextValue)) {
        throw new Error('[Zenith Runtime] state.set(next) must resolve to a plain object');
      }
      const frozen = Object.freeze({ ...nextValue });
      if (Object.is(current, frozen)) return current;
      current = frozen;
      const snapshot = [...subscribers];
      for (let i = 0; i < snapshot.length; i++) snapshot[i](current);
      return current;
    },
    subscribe(fn) {
      if (typeof fn !== 'function') {
        throw new Error('[Zenith Runtime] state.subscribe(fn) requires a function');
      }
      subscribers.add(fn);
      return function unsubscribe() { subscribers.delete(fn); };
    }
  };
}

export function zeneffect(dependencies, fn) {
  if (!Array.isArray(dependencies) || dependencies.length === 0) {
    throw new Error('[Zenith Runtime] zeneffect(deps, fn) requires non-empty deps');
  }
  if (typeof fn !== 'function') {
    throw new Error('[Zenith Runtime] zeneffect(deps, fn) requires fn');
  }
  const unsubscribers = dependencies.map((dep, index) => {
    if (!dep || typeof dep.subscribe !== 'function') {
      throw new Error('[Zenith Runtime] zeneffect dependency at index ' + index + ' must expose subscribe(fn)');
    }
    return dep.subscribe(() => fn());
  });
  fn();
  return function dispose() {
    for (let i = 0; i < unsubscribers.length; i++) unsubscribers[i]();
  };
}
"#
    .to_string()
}

fn upsert_router_manifest(out_dir: &PathBuf, entry: RouterRouteEntry) -> Result<(), String> {
    let manifest_path = out_dir.join("assets").join("router-manifest.json");
    if let Some(parent) = manifest_path.parent() {
        fs::create_dir_all(parent).map_err(|e| {
            format!(
                "failed to create router manifest dir '{}': {e}",
                parent.display()
            )
        })?;
    }

    let mut manifest = if manifest_path.exists() {
        let source = fs::read_to_string(&manifest_path).map_err(|e| {
            format!(
                "failed to read router manifest '{}': {e}",
                manifest_path.display()
            )
        })?;
        serde_json::from_str::<RouterManifest>(&source)
            .map_err(|e| format!("invalid router manifest '{}': {e}", manifest_path.display()))?
    } else {
        RouterManifest::default()
    };

    if let Some(existing) = manifest
        .routes
        .iter_mut()
        .find(|route| route.path == entry.path)
    {
        *existing = entry;
    } else {
        manifest.routes.push(entry);
    }

    manifest.routes.sort_by(|a, b| a.path.cmp(&b.path));

    let json = serde_json::to_string(&manifest)
        .map_err(|e| format!("failed to serialize router manifest: {e}"))?;
    fs::write(&manifest_path, json).map_err(|e| {
        format!(
            "failed to write router manifest '{}': {e}",
            manifest_path.display()
        )
    })?;

    Ok(())
}

fn generate_router_runtime_js() -> String {
    r#"(function() {
  const MANIFEST_URL = '/assets/router-manifest.json';
  let manifestPromise = null;

  function loadManifest() {
    if (!manifestPromise) {
      manifestPromise = fetch(MANIFEST_URL, { cache: 'no-store' })
        .then((res) => (res.ok ? res.json() : { routes: [] }))
        .catch(() => ({ routes: [] }));
    }
    return manifestPromise;
  }

  function splitPath(path) {
    return path.split('/').filter(Boolean);
  }

  function matchRoute(pathname, routes) {
    const segments = splitPath(pathname);
    for (let i = 0; i < routes.length; i++) {
      const route = routes[i];
      const routeSegs = splitPath(route.path);
      if (routeSegs.length !== segments.length) continue;

      const params = {};
      let matched = true;
      for (let j = 0; j < routeSegs.length; j++) {
        const routeSeg = routeSegs[j];
        const seg = segments[j];
        if (routeSeg.startsWith(':')) {
          params[routeSeg.slice(1)] = seg;
          continue;
        }
        if (routeSeg !== seg) {
          matched = false;
          break;
        }
      }
      if (matched) return { route, params };
    }
    return null;
  }

  function resolveExpression(expr, params) {
    const match = /^params\.([A-Za-z_$][\w$]*)$/.exec(expr);
    if (!match) return '';
    const value = params[match[1]];
    return value == null ? '' : String(value);
  }

  function renderRoute(match) {
    const template = document.createElement('template');
    template.innerHTML = match.route.html;

    const nodes = template.content.querySelectorAll('[data-zx-e]');
    for (let i = 0; i < nodes.length; i++) {
      const node = nodes[i];
      const raw = node.getAttribute('data-zx-e') || '';
      const parts = raw.split(/\s+/).filter(Boolean);
      let text = '';

      for (let j = 0; j < parts.length; j++) {
        const idx = Number(parts[j]);
        if (!Number.isInteger(idx)) continue;
        if (idx < 0 || idx >= match.route.expressions.length) continue;
        text += resolveExpression(match.route.expressions[idx], match.params);
      }

      node.textContent = text;
    }

    const all = template.content.querySelectorAll('*');
    for (let i = 0; i < all.length; i++) {
      const attrs = all[i].attributes;
      for (let j = attrs.length - 1; j >= 0; j--) {
        const name = attrs[j].name;
        if (name.startsWith('data-zx-on-')) {
          all[i].removeAttribute(name);
        }
      }
    }

    const container = document.getElementById('app');
    if (container) {
      container.innerHTML = '';
      container.appendChild(template.content.cloneNode(true));
      return;
    }

    document.body.innerHTML = '';
    document.body.appendChild(template.content.cloneNode(true));
  }

  async function resolvePath(pathname) {
    const manifest = await loadManifest();
    const routes = Array.isArray(manifest.routes) ? manifest.routes : [];
    const matched = matchRoute(pathname, routes);
    if (!matched) return false;
    renderRoute(matched);
    return true;
  }

  function isInternalLink(anchor) {
    if (!anchor || anchor.target || anchor.hasAttribute('download')) return false;
    const href = anchor.getAttribute('href');
    if (!href || href.startsWith('#') || href.startsWith('mailto:') || href.startsWith('tel:')) {
      return false;
    }
    const url = new URL(anchor.href, window.location.href);
    return url.origin === window.location.origin;
  }

  async function navigate(pathname) {
    const ok = await resolvePath(pathname);
    if (!ok) {
      window.location.assign(pathname);
      return;
    }
    history.pushState({}, '', pathname);
  }

  document.addEventListener('click', function(event) {
    const target = event.target && event.target.closest ? event.target.closest('a[href]') : null;
    if (!isInternalLink(target)) return;

    const url = new URL(target.href, window.location.href);
    const nextPath = url.pathname;
    if (nextPath === window.location.pathname) return;

    event.preventDefault();
    navigate(nextPath);
  });

  window.addEventListener('popstate', function() {
    resolvePath(window.location.pathname);
  });

  loadManifest().then((manifest) => {
    const routes = Array.isArray(manifest.routes) ? manifest.routes : [];
    const initial = matchRoute(window.location.pathname, routes);
    if (initial && initial.route && typeof initial.route.path === 'string' && initial.route.path.includes(':')) {
      renderRoute(initial);
    }
  });
})();"#
        .to_string()
}
