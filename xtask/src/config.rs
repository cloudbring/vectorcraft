use std::path::Path;

use craft_xtask::{Cargo, Result, ico, layers, pdf_deps, stats, version};

pub fn version(root: &Path) -> version::Config {
    version::Config { cargo: Cargo { root: root.into(), ..Cargo::default() }, ..version::Config::default() }
}

pub fn stats(root: &Path) -> stats::Config {
    stats::Config { cargo: Cargo { root: root.into(), ..Cargo::default() }, follow_symlinks: true, ..stats::Config::default() }
}

fn names(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).into()).collect()
}

pub fn layers(root: &Path) -> layers::Config {
    layers::Config {
        root: root.into(),
        layers: [
            ("L0", &["geom", "color", "affinity", "scriptcheck"][..]),
            ("L1", &["doc"]),
            ("L2", &["pathops", "brush", "trace", "text", "effects", "three-d", "plugins"]),
            ("L3", &["render", "svg", "pdf", "format", "cad", "eps", "metafile"]),
            ("L4", &["tools"]),
            ("L5", &["engine"]),
            ("L6", &["ui-egui", "mcp", "testkit"]),
        ]
        .into_iter()
        .map(|(label, members)| (label.into(), members.iter().map(|name| format!("vectorcraft-{name}")).collect()))
        .collect(),
        exempt: names(&["vectorcraft", "vectorcraft-cli", "vectorcraft-web", "xtask"]),
        standalone: names(&["vectorcraft-affinity", "vectorcraft-scriptcheck"]),
        testkits: names(&["vectorcraft-testkit"]),
        allowed_edges: [
            ("color", "geom"),
            ("effects", "pathops"),
            ("trace", "pathops"),
            ("effects", "text"),
            ("eps", "render"),
            ("eps", "metafile"),
            ("eps", "pdf"),
            ("effects", "plugins"),
            ("effects", "three-d"),
            ("three-d", "pathops"),
        ]
        .into_iter()
        .map(|(source, dest)| (format!("vectorcraft-{source}"), format!("vectorcraft-{dest}")))
        .collect(),
        external_rules: vec![layers::ExternalRule {
            dependencies: names(&["egui", "eframe", "winit", "egui_kittest", "craft-ui", "craft-keymap", "rfd", "bevy*"]),
            allowed_from: names(&["vectorcraft-ui-egui", "vectorcraft-mcp", "vectorcraft-testkit"]),
            allowed_dev_from: Vec::new(),
        }],
        ..layers::Config::default()
    }
}

pub fn wasm_safe(policy: &layers::Config, name: &str) -> bool {
    policy.layers.iter().take(6).any(|(_, members)| members.iter().any(|member| member == name))
        || matches!(name, "vectorcraft-ui-egui" | "vectorcraft-web")
}

fn describe(policy: &layers::Config, name: &str) -> String {
    if policy.exempt.iter().any(|member| member == name) {
        "exempt".into()
    } else if policy.standalone.iter().any(|member| member == name) {
        "L0 standalone".into()
    } else if policy.testkits.iter().any(|member| member == name) {
        "testkit".into()
    } else {
        policy
            .layers
            .iter()
            .find(|(_, members)| members.iter().any(|member| member == name))
            .map(|(label, _)| label.clone())
            .unwrap_or("UNREGISTERED".into())
    }
}

pub fn layer_report(policy: &layers::Config, metadata: &serde_json::Value) -> Result<(String, Result)> {
    let crates = layers::from_metadata(metadata)?;
    let mut report = format!("Dependency layering (plan/architecture.md §3)\n\n{:<28} {:<14} workspace deps\n", "crate", "layer");
    for krate in &crates {
        let deps = krate
            .deps
            .iter()
            .filter(|dep| dep.workspace)
            .map(|dep| {
                let suffix = match dep.kind {
                    layers::DepKind::Normal => "",
                    layers::DepKind::Dev => " (dev)",
                    layers::DepKind::Build => " (build)",
                };
                format!("{}{suffix}", dep.name.strip_prefix("vectorcraft-").unwrap_or(&dep.name))
            })
            .collect::<Vec<_>>();
        report += &format!("{:<28} {:<14} {}\n", krate.name, describe(policy, &krate.name), deps.join(", "));
    }
    let violations = layers::check(policy, &crates)?;
    report += "\n";
    let result = if violations.is_empty() {
        report += &format!("OK: {} crates, no layering violations.\n", crates.len());
        Ok(())
    } else {
        report += &format!("{} violation(s):\n", violations.len());
        for violation in &violations {
            report += &format!("  - {violation}\n");
        }
        Err(format!("{} layering violation(s)", violations.len()))
    };
    Ok((report, result))
}

pub const PDF_REV: &str = "92d392cff132eaf53e13afff77f38b5c11b73aa0";
pub const KRILLA_REV: &str = "fb08be5fa19eee8d37e624397fcdc96df6035e92";

pub fn pdf_deps(root: &Path) -> pdf_deps::Config {
    pdf_deps::Config {
        cargo: Cargo { root: root.into(), ..Cargo::default() },
        packages: [
            ("craft-pdf", "0.1.0", PDF_REV),
            ("krilla", "0.8.2", KRILLA_REV),
            ("hayro-syntax", "0.7.2", PDF_REV),
            ("hayro-interpret", "0.7.0", PDF_REV),
            ("hayro-jbig2", "0.3.0", PDF_REV),
            ("hayro-cmap", "0.1.0", PDF_REV),
            ("hayro-postscript", "0.1.0", PDF_REV),
        ]
        .into_iter()
        .map(|(name, version, revision)| pdf_deps::PackagePin {
            name: name.into(),
            version: version.into(),
            source: pdf_deps::git_source("https://github.com/storytold/craft-libs", revision),
            required_features: Vec::new(),
            exact_features: None,
        })
        .collect(),
        ..pdf_deps::Config::default()
    }
}

pub fn run_pdf_deps(policy: &pdf_deps::Config) -> Result {
    pdf_deps::validate(policy, &policy.cargo.metadata(true)?)?;
    println!("PDF: shared crate and patched backends resolve once to {PDF_REV} (krilla to {KRILLA_REV})");
    Ok(())
}

pub fn run_ico(policy: &ico::Config, args: &[String]) -> Result {
    let [output, inputs @ ..] = args else {
        return Err("usage: cargo xtask ico <out.ico> <png>...".into());
    };
    if inputs.is_empty() {
        return Err("usage: cargo xtask ico <out.ico> <png>...".into());
    }
    let pngs =
        inputs.iter().map(|path| std::fs::read(policy.root.join(path)).map_err(|error| format!("{path}: {error}"))).collect::<Result<Vec<_>>>()?;
    std::fs::write(policy.root.join(output), ico::pack_ico(&pngs)?).map_err(|error| format!("{output}: {error}"))
}
