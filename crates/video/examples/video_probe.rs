//! Prints what the video probe finds on this machine, and the encoder
//! chosen per GPU (plan Task 3).
//!
//! `cargo run -p twicecat-video --example video_probe [-- --sessions N]`;
//! `--sessions N` also counts how many NVENC sessions the first NVIDIA GPU
//! opens at once, up to N.

use twicecat_video::probe::{self, Gpu};

fn level(level: u32, h264: bool) -> String {
    if h264 {
        format!("{}.{}", level / 10, level % 10)
    } else {
        format!("{}.{}", level / 30, (level % 30) / 3)
    }
}

fn print(gpu: &Gpu) {
    println!("{} ({:?}, {:?})", gpu.name, gpu.vendor, gpu.kind);
    if let Some((major, minor)) = gpu.render_node {
        println!("  render node {major}:{minor}");
    }
    let v = &gpu.vulkan;
    println!(
        "  Vulkan {}.{}.{}, {}; encode queue: {}; H.264 decode: {}; DMA-BUF import: {}; fd export: {}; AV1 encode extension: {}",
        v.api_version.0,
        v.api_version.1,
        v.api_version.2,
        v.driver,
        v.encode_queue,
        v.h264_decode,
        v.dma_buf_import,
        v.external_memory_fd,
        v.av1_extension,
    );
    for (name, caps, h264) in [("H.264", &v.h264, true), ("H.265", &v.h265, false)] {
        match caps {
            Some(c) => println!(
                "  Vulkan Video {name}: up to {}×{}, level {}, {} Mb/s, CBR {} VBR {}, {} DPB slots, {} active references",
                c.max_width,
                c.max_height,
                level(c.max_level, h264),
                c.max_bitrate / 1_000_000,
                c.cbr,
                c.vbr,
                c.max_dpb_slots,
                c.max_active_references,
            ),
            None => println!("  Vulkan Video {name}: no"),
        }
    }
    if let Some(n) = &gpu.nvidia {
        println!(
            "  CUDA compute capability {}.{}, driver CUDA {}.{}; NVENC API {:?}; Vulkan → CUDA memory: {:?}",
            n.compute_capability.0,
            n.compute_capability.1,
            n.cuda_driver_version / 1000,
            n.cuda_driver_version % 1000 / 10,
            n.nvenc_api,
            n.vulkan_interop,
        );
        for (name, caps) in [
            ("H.264", n.nvenc_h264),
            ("H.265", n.nvenc_h265),
            ("AV1", n.nvenc_av1),
        ] {
            match caps {
                Some(c) => println!(
                    "  NVENC {name}: up to {}×{}, level {} (NV_ENC_LEVEL), {} macroblocks/s stated, {} temporal layers, dynamic bitrate {}, {} engines",
                    c.max_width,
                    c.max_height,
                    c.max_level,
                    c.max_mb_per_sec,
                    c.max_temporal_layers,
                    c.dynamic_bitrate,
                    c.engines,
                ),
                None => println!("  NVENC {name}: no"),
            }
        }
        for (name, caps) in [("H.264", n.nvdec_h264), ("AV1", n.nvdec_av1)] {
            match caps {
                Some(c) => println!(
                    "  NVDEC {name}: up to {}×{}, {} engines",
                    c.max_width, c.max_height, c.engines
                ),
                None => println!("  NVDEC {name}: no"),
            }
        }
    }
    println!("  chosen encoder: {:?}", probe::choose(gpu));
}

fn main() {
    let started = std::time::Instant::now();
    let found = probe::probe();
    let took = started.elapsed();
    if let Some(why) = found.vulkan_error {
        println!("Vulkan: {why}");
    }
    for gpu in &found.gpus {
        print(gpu);
    }
    println!("probe took {:.0} ms", took.as_secs_f64() * 1000.0);
    let mut args = std::env::args().skip(1);
    if args.next().as_deref() == Some("--sessions") {
        let limit = args.next().and_then(|n| n.parse().ok()).unwrap_or(16);
        #[cfg(any(target_os = "linux", target_os = "windows"))]
        println!(
            "NVENC sessions open at once (up to {limit}): {:?}",
            probe::nvenc_session_limit(limit)
        );
        #[cfg(not(any(target_os = "linux", target_os = "windows")))]
        println!("NVENC sessions (up to {limit}): no NVENC on this OS");
    }
}
