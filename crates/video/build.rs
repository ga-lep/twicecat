//! Compiles the compute shaders (`shaders/*.wgsl`) to SPIR-V with naga and
//! validates them (plan Q11, Task 4). The SPIR-V lands in `OUT_DIR` and is
//! included by `src/gpu/convert.rs`.

use std::{env, fs, path::PathBuf};

use naga::{
    ShaderStage,
    back::spv,
    valid::{Capabilities, ValidationFlags, Validator},
};

const SHADERS: [&str; 2] = ["bgra_to_nv12", "nv12_scale"];

fn main() {
    let out = PathBuf::from(env::var_os("OUT_DIR").expect("cargo sets OUT_DIR"));
    for name in SHADERS {
        let path = format!("shaders/{name}.wgsl");
        println!("cargo::rerun-if-changed={path}");
        let source = fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
        let module = naga::front::wgsl::parse_str(&source)
            .unwrap_or_else(|e| panic!("{}", e.emit_to_string_with_path(&source, &path)));
        let info = Validator::new(ValidationFlags::all(), Capabilities::PUSH_CONSTANT)
            .validate(&module)
            .unwrap_or_else(|e| panic!("{}", e.emit_to_string_with_path(&source, &path)));
        let options = spv::Options {
            // Vulkan 1.3 takes SPIR-V up to 1.6; 1.3 is enough here.
            lang_version: (1, 3),
            flags: spv::WriterFlags::empty(),
            ..spv::Options::default()
        };
        let pipeline = spv::PipelineOptions {
            shader_stage: ShaderStage::Compute,
            entry_point: "main".into(),
        };
        let words = spv::write_vec(&module, &info, &options, Some(&pipeline))
            .unwrap_or_else(|e| panic!("{path}: {e}"));
        let bytes: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
        fs::write(out.join(format!("{name}.spv")), bytes)
            .unwrap_or_else(|e| panic!("{name}.spv: {e}"));
    }
}
