use serde_json::{Value, json};

use crate::manifest::{Manifest, version_cmp};
use crate::{Host, Package, registry, testing};

/// Remembers the requests it gets and answers `host.echo` with its params.
#[derive(Default)]
struct Recorder {
    ops: Vec<String>,
    logs: Vec<String>,
}

impl Host for Recorder {
    fn call(&mut self, op: &str, params: &Value) -> Result<Value, String> {
        self.ops.push(op.to_string());
        match op {
            "host.echo" => Ok(params.clone()),
            _ => Err(format!("no '{op}'")),
        }
    }
    fn log(&mut self, text: &str) {
        self.logs.push(text.to_string());
    }
}

fn load(id: &str, perms: &[&str]) -> std::sync::Arc<crate::Plugin> {
    let pkg = Package::from_zip(&testing::package(id, perms)).unwrap();
    crate::load(pkg, "test").unwrap()
}

#[test]
fn packages_load_and_commands_run() {
    let p = load("unit-hello", &[]);
    assert_eq!(p.manifest.commands.len(), 6);
    let cmd = crate::command("plugin:unit-hello/h").unwrap();
    assert!(!cmd.mutates(), "no edit permission: a view command");
    let mut host = Recorder::default();
    assert_eq!(crate::run_command(&cmd, &mut host, &json!({})).unwrap(), json!({"hello": "world"}));
    assert_eq!(host.logs, vec!["hi"]);
    // Errors come back as text.
    let e = crate::command("plugin:unit-hello/e").unwrap();
    assert!(crate::run_command(&e, &mut host, &Value::Null).unwrap_err().contains("nope"));
    assert!(crate::commands().iter().any(|c| c.id == "plugin:unit-hello/f"));
    assert!(crate::unload("unit-hello"));
    assert!(crate::command("plugin:unit-hello/h").is_none());
}

#[test]
fn plugins_reach_the_host_through_requests() {
    load("unit-forward", &["edit"]);
    let cmd = crate::command("plugin:unit-forward/f").unwrap();
    assert!(cmd.mutates());
    let mut host = Recorder::default();
    let v = crate::run_command(&cmd, &mut host, &json!({"op": "host.echo", "params": {"a": [1, 2]}})).unwrap();
    assert_eq!(v, json!({"a": [1, 2]}));
    let e = crate::run_command(&cmd, &mut host, &json!({"op": "nope", "params": {}})).unwrap_err();
    assert!(e.contains("no 'nope'"), "{e}");
    assert_eq!(host.ops, vec!["host.echo", "nope"]);
    crate::unload("unit-forward");
}

#[test]
fn hostile_modules_are_stopped_not_crashes() {
    load("unit-hostile", &[]);
    let mut host = Recorder::default();
    for (id, want) in [("s", "too long"), ("g", "out of bounds"), ("o", "error")] {
        let cmd = crate::command(&format!("plugin:unit-hostile/{id}")).unwrap();
        let e = crate::run_command(&cmd, &mut host, &json!({})).unwrap_err();
        assert!(e.contains(want), "{id}: {e}");
    }
    crate::unload("unit-hostile");
}

#[test]
fn bad_packages_are_refused() {
    let wasm = testing::wasm();
    let man = |v: Value| testing::zip_of(&[("plugin.json", v.to_string().as_bytes()), ("plugin.wasm", &wasm)]);
    let ok = json!({"id": "x-ok", "name": "X", "version": "1.0.0"});
    assert!(Package::from_zip(&man(ok.clone())).is_ok());
    for (bad, why) in [
        (json!({"id": "Bad Id", "name": "X", "version": "1.0.0"}), "id"),
        (json!({"id": "x", "name": "", "version": "1.0.0"}), "name"),
        (json!({"id": "x", "name": "X", "version": "one"}), "version"),
        (json!({"id": "x", "name": "X", "version": "1.0.0", "entry": "../evil.wasm"}), "entry"),
        (json!({"id": "x", "name": "X", "version": "1.0.0", "abi": 99}), "ABI"),
        (json!({"id": "x", "name": "X", "version": "1.0.0", "minRustgisVersion": "999.0.0"}), "needs RustGIS"),
        (json!({"id": "x", "name": "X", "version": "1.0.0", "permissions": ["root"]}), "plugin.json"),
        (
            json!({"id": "x", "name": "X", "version": "1.0.0", "tools": [{"id": "t", "name": "T", "params": [{"name": "a", "label": "A", "kind": "laser"}]}]}),
            "kind",
        ),
        (
            json!({"id": "x", "name": "X", "version": "1.0.0", "commands": [{"id": "c", "label": "C"}, {"id": "c", "label": "D"}]}),
            "repeated",
        ),
    ] {
        let e = Package::from_zip(&man(bad.clone())).unwrap_err();
        assert!(e.contains(why), "{bad}: {e}");
    }
    assert!(Package::from_zip(b"not a zip").is_err());
    assert!(Package::from_zip(&testing::zip_of(&[("readme.txt", b"hi")])).unwrap_err().contains("no plugin.json"));
    // A package in one top-level folder is fine.
    let nested = testing::zip_of(&[("x/plugin.json", ok.to_string().as_bytes()), ("x/plugin.wasm", &wasm)]);
    assert!(Package::from_zip(&nested).is_ok());
    // A module that isn't one, or lacks the ABI's exports, doesn't load.
    let junk = testing::zip_of(&[("plugin.json", ok.to_string().as_bytes()), ("plugin.wasm", b"\0asm junk")]);
    assert!(crate::load(Package::from_zip(&junk).unwrap(), "t").is_err());
    let empty = wat::parse_str("(module)").unwrap();
    let e = testing::zip_of(&[("plugin.json", ok.to_string().as_bytes()), ("plugin.wasm", &empty)]);
    assert!(crate::load(Package::from_zip(&e).unwrap(), "t").unwrap_err().contains("doesn't export"));
    let wasi = wat::parse_str(r#"(module (import "wasi_snapshot_preview1" "fd_write" (func (param i32 i32 i32 i32) (result i32))) (memory (export "memory") 1) (func (export "rustgis_alloc") (param i32) (result i32) i32.const 0) (func (export "rustgis_call") (param i32 i32) (result i64) i64.const 0))"#).unwrap();
    let e = testing::zip_of(&[("plugin.json", ok.to_string().as_bytes()), ("plugin.wasm", &wasi)]);
    assert!(crate::load(Package::from_zip(&e).unwrap(), "t").unwrap_err().contains("doesn't provide"));
}

#[test]
fn tools_join_the_geoprocessing_tools() {
    crate::tools::register();
    load("unit-tools", &[]);
    let spec = rustgis_analysis::tool("plugin:unit-tools/m").unwrap();
    assert_eq!(spec.toolbox, "Plugins › Test unit-tools");
    assert_eq!(rustgis_analysis::external_source(spec), Some(crate::tools::SOURCE));
    assert!(matches!(spec.params.get(2).map(|p| p.kind), Some(rustgis_analysis::ParamKind::Choice(c)) if c == ["a", "b"]));
    struct One(rustgis_data::Dataset);
    impl rustgis_analysis::Inputs for One {
        fn dataset(&self, l: &str) -> Option<&rustgis_data::Dataset> {
            (l == "Cities").then_some(&self.0)
        }
        fn selection(&self, _: &str) -> &[usize] {
            &[]
        }
    }
    let (ds, _) = rustgis_data::io::read_geojson(r#"{"type":"Point","coordinates":[3,4]}"#, "Cities").unwrap();
    let inputs = One(ds);
    let r = rustgis_analysis::run("plugin:unit-tools/m", &inputs, &json!({"input": "Cities"})).unwrap();
    assert_eq!(r.messages, vec!["made"]);
    match r.output {
        rustgis_analysis::Output::Layers(v) => {
            assert!(matches!(v.as_slice(), [rustgis_analysis::OutputLayer::Vector(d)] if d.name == "Made" && d.features.len() == 1))
        }
        o => panic!("{o:?}"),
    }
    let r = rustgis_analysis::run("plugin:unit-tools/x", &inputs, &json!({"input": "Cities"})).unwrap();
    assert!(matches!(r.output, rustgis_analysis::Output::Selection { ref layer, ref rows } if layer == "Cities" && rows == &[0, 2]));
    assert!(rustgis_analysis::run("plugin:unit-tools/m", &inputs, &json!({"input": "Nope"})).unwrap_err().contains("no layer"));
    // Disabled plugins offer no tools.
    crate::set_enabled("unit-tools", false).unwrap();
    assert!(rustgis_analysis::tool("plugin:unit-tools/m").is_none());
    crate::unload("unit-tools");
}

#[test]
fn registries_and_blocklists_are_read_defensively() {
    let sha = "a".repeat(64);
    let text = json!({"version": 1, "plugins": [
        {"id": "b-plugin", "name": "B", "description": "Long", "version": "1.2.0", "package": "packages/b/b-1.2.0.rgplugin", "sha256": sha,
         "i18n": {"de": {"B": "Be", "Other": "Andere", "Long": "x".repeat(5000)}, "fr": {"Other": "Autre"}}},
        {"id": "a-plugin", "name": "A", "version": "0.1.0", "package": "https://example.com/a.rgplugin", "sha256": sha, "homepage": "javascript:alert(1)"},
        {"id": "bad", "name": "Bad", "version": "1.0.0", "package": "http://insecure.example.com/x.rgplugin", "sha256": sha},
        {"id": "nohash", "name": "No hash", "version": "1.0.0", "package": "x.rgplugin", "sha256": "abc"},
        {"id": "Bad Id", "name": "X", "version": "1.0.0", "package": "x.rgplugin", "sha256": sha},
        7
    ]})
    .to_string();
    let v = registry::parse(&text, "https://plugins.rustgis.app/registry.json").unwrap();
    assert_eq!(v.iter().map(|e| e.id.as_str()).collect::<Vec<_>>(), ["a-plugin", "b-plugin"]);
    assert_eq!(v[1].package, "https://plugins.rustgis.app/packages/b/b-1.2.0.rgplugin");
    assert!(v[0].homepage.is_empty(), "only http(s) homepages");
    // Only the name and description keep translations.
    assert_eq!(v[1].translate("de", "B"), "Be");
    assert_eq!(v[1].i18n.get("de").map(|t| t.len()), Some(1));
    assert!(!v[1].i18n.contains_key("fr"));
    // Languages with nothing to keep don't use up the 64 allowed: `de` sorts after 70 of them.
    let mut i18n = serde_json::Map::new();
    for i in 0..70 {
        i18n.insert(format!("a{i:02}"), json!({"Other": "x"}));
    }
    i18n.insert("de".into(), json!({"C": "Ce"}));
    let one = json!([{"id": "c-plugin", "name": "C", "version": "1.0.0", "package": "c.rgplugin", "sha256": sha, "i18n": i18n}]);
    let v = registry::parse(&one.to_string(), "https://plugins.rustgis.app/registry.json").unwrap();
    assert_eq!(v[0].translate("de", "C"), "Ce");
    assert_eq!(v[0].i18n.len(), 1);
    assert!(registry::parse("[]", "x").unwrap().is_empty());
    assert!(registry::parse("{", "x").is_err());
    assert_eq!(registry::resolve("http://localhost:8000/r/registry.json", "/p.rgplugin"), "http://localhost:8000/p.rgplugin");
    for (url, ok) in [
        ("https://plugins.rustgis.app/x", true),
        ("http://localhost:8000/registry.json", true),
        ("http://127.0.0.1/registry.json", true),
        ("http://localhost", true),
        ("http://localhost.evil.com/x", false),
        ("http://127.0.0.1.evil.com/x", false),
        ("http://localhost:80@evil.example/pkg.rgplugin", false),
        ("http://127.0.0.1:x@evil.example/", false),
        ("http://localhost:8000/a@b.rgplugin", true),
        ("http://example.com/x", false),
        ("file:///etc/passwd", false),
    ] {
        assert_eq!(registry::allowed_url(url), ok, "{url}");
    }
    assert_eq!(registry::blocklist_url("https://plugins.rustgis.app/registry.json"), "https://plugins.rustgis.app/blocklist.json");
    let list =
        registry::parse_blocklist(&json!({"blocked": [{"id": "x", "reason": "bad"}, {"id": "y", "sha256": sha}]}).to_string()).unwrap();
    assert!(registry::blocked(&list, "x", "anything").is_some());
    assert!(registry::blocked(&list, "y", &sha).is_some());
    assert!(registry::blocked(&list, "y", &"b".repeat(64)).is_none());
}

#[test]
fn blocked_plugins_are_unloaded_and_refused() {
    load("unit-blocked", &[]);
    let pulled = crate::set_blocklist(vec![registry::Blocked { id: "unit-blocked".into(), sha256: None, reason: "malware".into() }]);
    assert_eq!(pulled, vec!["Test unit-blocked"]);
    assert!(crate::get("unit-blocked").is_none());
    let pkg = Package::from_zip(&testing::package("unit-blocked", &[])).unwrap();
    assert!(crate::load(pkg, "t").unwrap_err().contains("malware"));
    crate::set_blocklist(Vec::new());
    // Another registry's blocklist only adds: it can't bring back what the hosted one withdrew.
    load("unit-blocked", &[]);
    let pulled = crate::apply_blocklist(
        "https://example.com/registry.json",
        vec![registry::Blocked { id: "unit-blocked".into(), sha256: None, reason: "x".into() }],
    );
    assert_eq!(pulled, vec!["Test unit-blocked"]);
    crate::apply_blocklist("https://example.com/registry.json", Vec::new());
    assert!(crate::blocked("unit-blocked", "any").is_some(), "an empty list from another registry unblocks nothing");
}

#[test]
fn versions_and_translations() {
    assert!(version_cmp("1.10.0", "1.9.3").is_gt());
    assert!(version_cmp("v2.0", "2.0.0").is_eq());
    assert!(version_cmp("1.0.0-beta.1", "1.0.0").is_eq());
    let m = Manifest::parse(&testing::manifest("tr", &[])).unwrap();
    assert_eq!(m.translate("de-AT", "Hello"), "Hallo");
    assert_eq!(m.translate("fr", "Hello"), "Hello");
    // The interface's `zh` finds a plugin's `zh-CN`; an exact match wins over another region.
    let mut m = m;
    m.i18n.insert("zh-TW".into(), [("Hello".to_string(), "哈囉".to_string())].into());
    m.i18n.insert("zh-CN".into(), [("Hello".to_string(), "你好".to_string())].into());
    assert_eq!(m.translate("zh", "Hello"), "你好");
    assert_eq!(m.translate("zh-TW", "Hello"), "哈囉");
    // Loaded plugins translate for the interface (tool names, parameters), by language.
    load("unit-i18n", &[]);
    assert_eq!(crate::translate("de", "Hello").as_deref(), Some("Hallo"));
    assert_eq!(crate::translate("fr", "Hello"), None);
    assert_eq!(crate::translate("de", "Not a plugin string"), None);
}

#[test]
fn plugin_tabs_are_checked() {
    let with_tab = |tab: Value| {
        let mut m: Value = serde_json::from_str(&testing::manifest("tabbed", &[])).unwrap();
        m["tab"] = tab;
        Manifest::parse(&m.to_string())
    };
    let ok = with_tab(json!({"label": "Hello", "groups": [
        {"label": "Run", "items": ["h", "tool:m", {"menu": "More", "icon": "stats", "items": ["f", "tool:x"]}]}
    ]}))
    .unwrap();
    let tab = ok.tab.as_ref().unwrap();
    assert!(matches!(tab.groups[0].items[2], crate::ItemDecl::Menu(ref m) if m.items.len() == 2));
    assert!(matches!(ok.entry("tool:m"), Some(crate::Entry::Tool(t)) if t.id == "m"));
    assert!(matches!(ok.entry("h"), Some(crate::Entry::Command(c)) if c.id == "h"));
    for (bad, why) in [
        (json!({"label": "Map", "groups": [{"label": "G", "items": ["h"]}]}), "RustGIS's tabs"),
        (json!({"label": "help", "groups": [{"label": "G", "items": ["h"]}]}), "RustGIS's tabs"),
        (json!({"label": "Style", "groups": [{"label": "G", "items": ["h"]}]}), "RustGIS's tabs"),
        (json!({"label": "publish", "groups": [{"label": "G", "items": ["h"]}]}), "RustGIS's tabs"),
        (json!({"label": "", "groups": [{"label": "G", "items": ["h"]}]}), "label"),
        (json!({"label": "T", "groups": []}), "groups"),
        (json!({"label": "T", "groups": [{"label": "G", "items": []}]}), "items"),
        (json!({"label": "T", "groups": [{"label": "G", "items": ["nope"]}]}), "isn't one of"),
        (json!({"label": "T", "groups": [{"label": "G", "items": ["tool:h"]}]}), "isn't one of"),
        (json!({"label": "T", "groups": [{"label": "G", "items": [{"menu": "M", "items": []}]}]}), "needs 1 to"),
        (json!({"label": "T", "groups": [{"label": "G", "items": [{"menu": "M", "items": ["zz"]}]}]}), "isn't one of"),
        (json!({"label": "T", "groups": [{"label": "G", "items": [7]}]}), "plugin.json"),
    ] {
        let e = with_tab(bad.clone()).unwrap_err();
        assert!(e.contains(why), "{bad}: {e}");
    }
}

#[test]
fn large_registries_are_read_quickly() {
    // 20,000 plugins (and a duplicate of each, which is dropped) parse in a fraction of a second;
    // the bound is generous so a busy CI machine doesn't fail it, while a quadratic scan
    // (minutes at this size) still would.
    let sha = "a".repeat(64);
    let one = |i: usize| json!({"id": format!("p{i}"), "name": format!("Plugin {i}"), "version": "1.0.0", "package": format!("https://packages.example.com/p{i}.rgplugin"), "sha256": sha});
    let list: Vec<Value> = (0..20_000).chain(0..20_000).map(one).collect();
    let text = json!({"plugins": list}).to_string();
    let t0 = std::time::Instant::now();
    let v = registry::parse(&text, "https://plugins.rustgis.app/registry.json").unwrap();
    assert_eq!(v.len(), 20_000);
    assert!(t0.elapsed() < std::time::Duration::from_secs(5), "{:?}", t0.elapsed());
}

#[test]
fn file_parameters_need_files_and_hand_tools_the_file() {
    use base64::Engine;
    let b64 = base64::engine::general_purpose::STANDARD;
    // Declared only with `files`, and with plain extensions.
    let mut m: Value = serde_json::from_str(&testing::manifest("file-param", &["files"])).unwrap();
    assert!(Manifest::parse(&m.to_string()).is_ok());
    m["permissions"] = json!([]);
    assert!(Manifest::parse(&m.to_string()).unwrap_err().contains("'files' permission"));
    m["permissions"] = json!(["files"]);
    for bad in [json!([".nc"]), json!([""]), json!(["n c"]), json!(vec!["nc"; 17])] {
        m["tools"][2]["params"][1]["extensions"] = bad.clone();
        assert!(Manifest::parse(&m.to_string()).unwrap_err().contains("extensions"), "{bad}");
    }
    let m = Manifest::parse(&testing::manifest("file-param", &["files"])).unwrap();
    let decl = m.tools.iter().find(|t| t.id == "r").unwrap();

    // A path is read; the parameter keeps the file's name and the contents go beside it.
    let dir = std::env::temp_dir().join(format!("rustgis-test-file-param-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("model.nc");
    std::fs::write(&path, b"CDF\x01").unwrap();
    let r = crate::tools::request(decl, &json!({"model": path.display().to_string()}), Default::default()).unwrap();
    assert_eq!(r["params"]["model"], "model.nc");
    assert_eq!(r["files"]["model"]["name"], "model.nc");
    assert_eq!(b64.decode(r["files"]["model"]["data"].as_str().unwrap()).unwrap(), b"CDF\x01");
    // A file picked in the browser comes with its contents, its name stripped of any folder.
    let picked = json!({"model": {"name": "C:\\runs\\pism.nc", "data": b64.encode("abc")}});
    let r = crate::tools::request(decl, &picked, Default::default()).unwrap();
    assert_eq!(r["params"]["model"], "pism.nc");
    assert_eq!(r["files"]["model"]["data"], b64.encode("abc"));
    // No file, no `files`; a missing file, a folder, a non-text value or too large a file is
    // an error, never a crash.
    assert!(crate::tools::request(decl, &json!({}), Default::default()).unwrap().get("files").is_none());
    for bad in [json!(dir.join("missing.nc").display().to_string()), json!(dir.display().to_string()), json!(7), json!({"data": 7})] {
        assert!(crate::tools::request(decl, &json!({"model": bad}), Default::default()).is_err(), "{bad}");
    }
    let huge = "A".repeat(crate::tools::MAX_FILE.div_ceil(3) * 4 + 4);
    assert!(
        crate::tools::request(decl, &json!({"model": {"name": "big.nc", "data": huge}}), Default::default())
            .unwrap_err()
            .contains("larger")
    );
    let _ = std::fs::remove_dir_all(&dir);
}
