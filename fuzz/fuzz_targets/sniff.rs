//! A file dropped without an extension: its type is sniffed from its bytes, then it is read as that.
#![no_main]

use libfuzzer_sys::fuzz_target;
use rustgis_data::io::{read_layers, sniff_extension};

fuzz_target!(|data: &[u8]| {
    if let Some(ext) = sniff_extension(data) {
        let _ = read_layers(&format!("fuzz.{ext}"), data);
    }
});
