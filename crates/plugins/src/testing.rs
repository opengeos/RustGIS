//! A plugin for tests, written in the WebAssembly text format (feature `testing`).
//!
//! It answers by the first letter of the command or tool id. Requests are serialized with
//! sorted keys, so the id starts at byte 15 (`{"abi":1,"id":"…`) and a one-letter command's
//! params start at byte 56 (`{"abi":1,"id":"f","kind":"command","lang":"en","params":…}`; every
//! interface language code is two letters).
//!
//! - `h` (hello): logs "hi" and returns `{"hello": "world"}`.
//! - `f` (forward): sends its params to RustGIS as a host request and returns the reply.
//! - `s` (spin): loops forever (stopped by the fuel limit).
//! - `g` (garbage): returns a reply outside its memory.
//! - `o` (out of memory): asks for 4 GB, then traps.
//! - `e` (error): fails with "nope".
//! - `m` (make, a tool): returns a point layer "Made" and a message.
//! - `x` (select, a tool): selects rows 0 and 2 of the layer "Cities".

use std::io::Write;

const WAT: &str = r#"
(module
  (import "rustgis" "call" (func $call (param i32 i32) (result i32)))
  (import "rustgis" "result" (func $result (param i32)))
  (import "rustgis" "log" (func $log (param i32 i32)))
  (memory (export "memory") 2)
  (global $heap (mut i32) (i32.const 8192))
  (data (i32.const 0) "{\22ok\22:{\22hello\22:\22world\22}}")
  (data (i32.const 64) "hi")
  (data (i32.const 128) "{\22error\22:\22nope\22}")
  (data (i32.const 256) "{\22ok\22:{\22layers\22:[{\22name\22:\22Made\22,\22geojson\22:{\22type\22:\22FeatureCollection\22,\22features\22:[{\22type\22:\22Feature\22,\22properties\22:{\22n\22:1},\22geometry\22:{\22type\22:\22Point\22,\22coordinates\22:[1,2]}}]}}],\22messages\22:[\22made\22]}}")
  (data (i32.const 1024) "{\22ok\22:{\22selection\22:{\22layer\22:\22Cities\22,\22rows\22:[0,2]}}}")
  (func $alloc (export "rustgis_alloc") (param $n i32) (result i32)
    (local $p i32)
    (local.set $p (global.get $heap))
    (global.set $heap (i32.add (global.get $heap) (local.get $n)))
    (if (i32.gt_u (global.get $heap) (i32.mul (memory.size) (i32.const 65536)))
      (then (drop (memory.grow (i32.add (i32.div_u (local.get $n) (i32.const 65536)) (i32.const 1))))))
    (local.get $p))
  (func $reply (param $p i32) (param $n i32) (result i64)
    (i64.or (i64.shl (i64.extend_i32_u (local.get $p)) (i64.const 32)) (i64.extend_i32_u (local.get $n))))
  (func (export "rustgis_call") (param $p i32) (param $n i32) (result i64)
    (local $c i32) (local $len i32) (local $buf i32)
    (local.set $c (i32.load8_u (i32.add (local.get $p) (i32.const 15))))
    (if (i32.eq (local.get $c) (i32.const 104)) (then
      (call $log (i32.const 64) (i32.const 2))
      (return (call $reply (i32.const 0) (i32.const 24)))))
    (if (i32.eq (local.get $c) (i32.const 102)) (then
      (local.set $len (call $call (i32.add (local.get $p) (i32.const 56)) (i32.sub (local.get $n) (i32.const 57))))
      (local.set $buf (call $alloc (local.get $len)))
      (call $result (local.get $buf))
      (return (call $reply (local.get $buf) (local.get $len)))))
    (if (i32.eq (local.get $c) (i32.const 115)) (then (loop $l (br $l))))
    (if (i32.eq (local.get $c) (i32.const 103)) (then (return (i64.const 0x7fff000000000010))))
    (if (i32.eq (local.get $c) (i32.const 111)) (then (drop (memory.grow (i32.const 65000))) unreachable))
    (if (i32.eq (local.get $c) (i32.const 109)) (then (return (call $reply (i32.const 256) (i32.const 195)))))
    (if (i32.eq (local.get $c) (i32.const 120)) (then (return (call $reply (i32.const 1024) (i32.const 52)))))
    (call $reply (i32.const 128) (i32.const 16)))
)
"#;

/// The test module.
pub fn wasm() -> Vec<u8> {
    wat::parse_str(WAT).unwrap_or_default()
}

/// A manifest for the test module, with the given id and permissions.
pub fn manifest(id: &str, permissions: &[&str]) -> String {
    serde_json::json!({
        "id": id,
        "name": format!("Test {id}"),
        "version": "1.0.0",
        "description": "A plugin for tests",
        "permissions": permissions,
        "commands": [
            {"id": "h", "label": "Hello", "icon": "star"},
            {"id": "f", "label": "Forward", "params": "a host request"},
            {"id": "s", "label": "Spin"},
            {"id": "g", "label": "Garbage"},
            {"id": "o", "label": "Out of memory"},
            {"id": "e", "label": "Error"}
        ],
        "tools": [
            {"id": "m", "name": "Make Points", "summary": "Makes a point", "params": [
                {"name": "input", "label": "Input", "kind": "vectorLayer", "required": true},
                {"name": "size", "label": "Size", "kind": "number", "default": 3},
                {"name": "mode", "label": "Mode", "kind": "choice", "choices": ["a", "b"], "default": "a"}
            ]},
            {"id": "x", "name": "Select Two", "params": [{"name": "input", "label": "Input", "kind": "layer"}]}
        ],
        "i18n": {"de": {"Hello": "Hallo"}}
    })
    .to_string()
}

/// A `.rgplugin` package of the test module.
pub fn package(id: &str, permissions: &[&str]) -> Vec<u8> {
    zip_of(&[("plugin.json", manifest(id, permissions).as_bytes()), ("plugin.wasm", &wasm())])
}

/// A zip archive of `files` (name, bytes), stored uncompressed.
pub fn zip_of(files: &[(&str, &[u8])]) -> Vec<u8> {
    let mut w = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let opts = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for (name, bytes) in files {
        if w.start_file(*name, opts).is_err() || w.write_all(bytes).is_err() {
            return Vec::new();
        }
    }
    w.finish().map(|c| c.into_inner()).unwrap_or_default()
}
