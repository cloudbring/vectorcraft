#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[path = "../src/config.rs"]
mod config;

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};

use craft_xtask::{ico, layers, pdf_deps, stats, version};
use serde_json::{Value, json};

static NEXT_TEMP: AtomicUsize = AtomicUsize::new(0);

struct Temp(PathBuf);

impl Temp {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("vectorcraft-70-{}-{}", std::process::id(), NEXT_TEMP.fetch_add(1, Ordering::Relaxed)));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn write(&self, path: impl AsRef<Path>, bytes: impl AsRef<[u8]>) {
        let path = self.0.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, bytes).unwrap();
    }
}

impl Drop for Temp {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}

fn metadata() -> Value {
    serde_json::from_str(include_str!("golden/metadata.json")).unwrap()
}

fn cli(args: &[&str]) -> Output {
    let output = Command::new(env!("CARGO_BIN_EXE_xtask")).args(args).output().unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    output
}

#[test]
fn layers_report_matches_original_tree_and_live_dispatch() {
    let (report, result) = config::layer_report(&config::layers(Path::new(".")), &metadata()).unwrap();
    result.unwrap();
    assert_eq!(report.as_bytes(), include_bytes!("golden/layers.stdout"));
    assert_eq!(cli(&["layers"]).stdout, include_bytes!("golden/layers.stdout"));
}

#[test]
fn layers_reject_upward_standalone_testkit_and_ui_edges() {
    let policy = config::layers(Path::new("."));
    let graph = layers::from_metadata(&metadata()).unwrap();
    for (source, target, workspace, kind) in [
        ("geom", "vectorcraft-doc", true, layers::DepKind::Normal),
        ("geom", "vectorcraft-color", true, layers::DepKind::Build),
        ("affinity", "vectorcraft-geom", true, layers::DepKind::Dev),
        ("scriptcheck", "vectorcraft-testkit", true, layers::DepKind::Dev),
        ("engine", "vectorcraft-testkit", true, layers::DepKind::Normal),
        ("render", "egui", false, layers::DepKind::Build),
        ("geom", "bevy_ecs", false, layers::DepKind::Dev),
        ("text", "craft-keymap", false, layers::DepKind::Normal),
        ("testkit", "vectorcraft-ui-egui", true, layers::DepKind::Normal),
    ] {
        let mut mutated = graph.clone();
        mutated.iter_mut().find(|krate| krate.name == format!("vectorcraft-{source}")).unwrap().deps.push(layers::Dep {
            name: target.into(),
            kind,
            workspace,
        });
        assert_eq!(layers::check(&policy, &mutated).unwrap().len(), 1, "{source} -> {target}");
    }
}

#[test]
fn layers_preserve_intra_layer_dev_self_and_exempt_rules() {
    let policy = config::layers(Path::new("."));
    let mut graph = layers::from_metadata(&metadata()).unwrap();
    for (source, target, kind) in
        policy.allowed_edges.iter().map(|(source, target)| (source.as_str(), target.as_str(), layers::DepKind::Normal)).chain([
            ("vectorcraft-geom", "vectorcraft-testkit", layers::DepKind::Dev),
            ("vectorcraft-affinity", "vectorcraft-affinity", layers::DepKind::Dev),
            ("xtask", "vectorcraft-ui-egui", layers::DepKind::Normal),
        ])
    {
        graph.iter_mut().find(|krate| krate.name == source).unwrap().deps.push(layers::Dep { name: target.into(), kind, workspace: true });
    }
    assert!(layers::check(&policy, &graph).unwrap().is_empty());
    graph.push(layers::Crate { name: "vectorcraft-unknown".into(), deps: vec![] });
    assert_eq!(layers::check(&policy, &graph).unwrap().len(), 1);
}

#[test]
fn wasm_selection_preserves_original_layer_boundary() {
    let policy = config::layers(Path::new("."));
    let graph = layers::from_metadata(&metadata()).unwrap();
    let excluded: Vec<_> = graph.iter().filter(|krate| !config::wasm_safe(&policy, &krate.name)).map(|krate| krate.name.as_str()).collect();
    assert_eq!(excluded, ["vectorcraft", "vectorcraft-cli", "vectorcraft-mcp", "vectorcraft-testkit", "xtask"]);
}

#[test]
fn pdf_output_matches_original_and_adapter() {
    if let Some(root) = std::env::var_os("VECTORCRAFT_GOLDEN_PDF_ROOT") {
        config::run_pdf_deps(&config::pdf_deps(Path::new(&root))).unwrap();
        std::process::exit(0);
    }
    assert_eq!(cli(&["pdf-deps"]).stdout, include_bytes!("golden/pdf-deps.stdout"));
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let output = child("pdf_output_matches_original_and_adapter", "VECTORCRAFT_GOLDEN_PDF_ROOT", root);
    assert_eq!(output, include_bytes!("golden/pdf-deps.stdout"));
}

fn pdf_metadata() -> Value {
    let packages: Vec<_> = [
        ("craft-pdf", "0.1.0"),
        ("krilla", "0.8.2"),
        ("hayro-syntax", "0.7.2"),
        ("hayro-interpret", "0.7.0"),
        ("hayro-jbig2", "0.3.0"),
        ("hayro-cmap", "0.1.0"),
        ("hayro-postscript", "0.1.0"),
    ]
    .into_iter()
    .map(|(name, version)| {
        let revision = if name == "krilla" { "fb08be5fa19eee8d37e624397fcdc96df6035e92" } else { "92d392cff132eaf53e13afff77f38b5c11b73aa0" };
        json!({"name":name, "version":version, "source":format!("git+https://github.com/storytold/craft-libs?rev={revision}#{revision}")})
    })
    .collect();
    json!({"packages":packages})
}

#[test]
fn pdf_pins_reject_missing_duplicate_source_and_version_drift() {
    let policy = config::pdf_deps(Path::new("."));
    pdf_deps::validate(&policy, &pdf_metadata()).unwrap();
    for index in 0..7 {
        let mut missing = pdf_metadata();
        missing["packages"].as_array_mut().unwrap().remove(index);
        assert!(pdf_deps::validate(&policy, &missing).is_err());
        let mut duplicate = pdf_metadata();
        let package = duplicate["packages"][index].clone();
        duplicate["packages"].as_array_mut().unwrap().push(package);
        assert!(pdf_deps::validate(&policy, &duplicate).is_err());
        for (field, value) in
            [("source", Value::Null), ("source", json!("registry+https://github.com/rust-lang/crates.io-index")), ("version", json!("0.99.0"))]
        {
            let mut changed = pdf_metadata();
            changed["packages"][index][field] = value;
            assert!(pdf_deps::validate(&policy, &changed).is_err());
        }
    }
    let mut unrelated = pdf_metadata();
    unrelated["packages"].as_array_mut().unwrap().push(json!({"name":"craft-print", "source":null}));
    pdf_deps::validate(&policy, &unrelated).unwrap();
}

fn child(test: &str, key: &str, root: &Path) -> Vec<u8> {
    let output = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", test, "--nocapture", "--quiet", "--test-threads=1"])
        .env(key, root)
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    output.stdout.strip_prefix(b"\nrunning 1 test\n").unwrap().to_vec()
}

#[test]
fn version_read_matches_original() {
    if let Some(root) = std::env::var_os("VECTORCRAFT_GOLDEN_READ_ROOT") {
        version::run(&config::version(Path::new(&root)), &[]).unwrap();
        std::process::exit(0);
    }
    let temp = Temp::new();
    temp.write("Cargo.toml", include_bytes!("golden/version-before.toml"));
    assert_eq!(child("version_read_matches_original", "VECTORCRAFT_GOLDEN_READ_ROOT", &temp.0), include_bytes!("golden/version.stdout"));
    assert_eq!(cli(&["version"]).stdout, include_bytes!("golden/version.stdout"));
}

#[test]
fn version_edits_manifest_and_lockfile_identically() {
    if let Some(root) = std::env::var_os("VECTORCRAFT_GOLDEN_VERSION_ROOT") {
        version::run(&config::version(Path::new(&root)), &["set".into(), "1.2.3-rc.1".into()]).unwrap();
        std::process::exit(0);
    }
    let temp = Temp::new();
    temp.write("Cargo.toml", include_bytes!("golden/version-before.toml"));
    temp.write("src/lib.rs", "");
    assert_eq!(
        child("version_edits_manifest_and_lockfile_identically", "VECTORCRAFT_GOLDEN_VERSION_ROOT", &temp.0),
        include_bytes!("golden/version-set.stdout")
    );
    assert_eq!(std::fs::read(temp.0.join("Cargo.toml")).unwrap(), include_bytes!("golden/version-after.toml"));
    assert_eq!(std::fs::read(temp.0.join("Cargo.lock")).unwrap(), include_bytes!("golden/version-after.lock"));
}

#[test]
fn version_keeps_parser_and_update_policy() {
    let policy = config::version(Path::new("."));
    assert!(policy.refresh_lockfile && policy.online_fallback);
    assert!(!policy.strict_prerelease && !policy.bounded_core && !policy.rollback_on_failure && !policy.update_path_pins);
    let input = "[workspace.package]\r\nversion = \"0.21.0\"\r\n";
    assert_eq!(version::rewrite(&policy, input, "1.2.3-01").unwrap(), "[workspace.package]\r\nversion = \"1.2.3-01\"\r\n");
    for invalid in ["", "1", "1.2", "01.2.3", "1.2.3-", "1.2.3+a"] {
        assert!(version::validate(invalid).is_err());
    }
}

#[test]
fn stats_replays_original_tree_counts_byte_for_byte() {
    let temp = Temp::new();
    let counts: Vec<(String, usize, usize, usize, usize)> = serde_json::from_str(include_str!("golden/stats-counts.json")).unwrap();
    let mut packages = Vec::new();
    for (name, files, lines, tests, props) in counts {
        let source = "#[test]\n".repeat(tests) + &"proptest! {}\n".repeat(props) + &"\n".repeat(lines - tests - props);
        temp.write(format!("{name}/src/0.rs"), source);
        for index in 1..files {
            temp.write(format!("{name}/src/{index}.rs"), "");
        }
        packages.push(json!({"name":name, "manifest_path":format!("{name}/Cargo.toml")}));
    }
    let report = stats::report(&config::stats(&temp.0), &json!({"packages":packages}), false).unwrap();
    assert_eq!(report.as_bytes(), include_bytes!("golden/stats.stdout"));
}

#[test]
fn stats_preserves_exclusions_and_bounds_traversal() {
    let temp = Temp::new();
    temp.write("src/lib.rs", "// #[test]\n#[test] fn works() {}\nproptest! {}\n");
    for excluded in ["target", "fuzz", ".hidden"] {
        temp.write(format!("{excluded}/ignored.rs"), "#[test]\n");
    }
    #[cfg(unix)]
    std::os::unix::fs::symlink(&temp.0, temp.0.join("cycle")).unwrap();
    let mut policy = config::stats(&temp.0);
    assert_eq!(stats::count(&policy, &temp.0).unwrap(), stats::Counts { files: 1, lines: 3, tests: 1, proptests: 1 });
    policy.max_entries = 0;
    assert!(stats::count(&policy, &temp.0).is_err());
}

fn hex_bytes(text: &str) -> Vec<u8> {
    text.trim().as_bytes().as_chunks::<2>().0.iter().map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap()).collect()
}

#[test]
fn ico_bytes_and_silent_stdout_match_original() {
    let temp = Temp::new();
    let pngs: Vec<String> = serde_json::from_str(include_str!("golden/pngs.json")).unwrap();
    let mut args = vec!["out.ico".to_owned()];
    for (index, png) in pngs.iter().enumerate() {
        let path = format!("{index}.png");
        temp.write(&path, hex_bytes(png));
        args.push(path);
    }
    config::run_ico(&ico::Config { root: temp.0.clone(), ..ico::Config::default() }, &args).unwrap();
    assert_eq!(std::fs::read(temp.0.join("out.ico")).unwrap(), hex_bytes(include_str!("golden/ico.hex")));
    let output = Command::new(env!("CARGO_BIN_EXE_xtask")).current_dir(&temp.0).arg("ico").args(args).output().unwrap();
    assert!(output.status.success());
    assert_eq!(output.stdout, include_bytes!("golden/ico.stdout"));
    assert_eq!(std::fs::read(temp.0.join("out.ico")).unwrap(), hex_bytes(include_str!("golden/ico.hex")));
}

#[test]
fn ico_rejects_malformed_images_and_missing_arguments() {
    assert!(ico::pack_ico(&[]).is_err());
    assert!(ico::pack_ico(&[b"nope".to_vec()]).is_err());
    let pngs: Vec<String> = serde_json::from_str(include_str!("golden/pngs.json")).unwrap();
    for size in [0u32, 17, 257] {
        let mut png = hex_bytes(&pngs[0]);
        png[16..20].copy_from_slice(&size.to_be_bytes());
        assert!(ico::pack_ico(&[png]).is_err());
    }
    assert_eq!(config::run_ico(&ico::Config::default(), &[]).unwrap_err(), "usage: cargo xtask ico <out.ico> <png>...");
}

#[test]
fn shared_hashes_match_original_byte_and_file_digests() {
    let temp = Temp::new();
    let mut bytes = String::new();
    let mut files = String::new();
    for input in [b"".as_slice(), b"abc", b"font bytes", b"png", &[0x61; 1000]] {
        temp.write("input", input);
        bytes += &format!("{}\n", craft_hash::sha256_hex(input));
        files += &format!("{}\n", craft_xtask::sha256_hex(&temp.0.join("input")).unwrap());
    }
    assert_eq!(bytes.as_bytes(), include_bytes!("golden/sha256.stdout"));
    assert_eq!(files.as_bytes(), include_bytes!("golden/sha256.stdout"));
}

#[test]
fn help_text_is_preserved() {
    let output = cli(&["help"]);
    let added = output.stdout.strip_prefix(include_bytes!("golden/help.stdout")).unwrap();
    assert_eq!(added, format!("\nShared task syntax:\n  {}\n  {}\n  {}\n", version::cli_help(), stats::cli_help(), ico::cli_help()).as_bytes());
}
