//! Hardware tests for our Vulkan code (plan Task 4): conversion, scaling
//! and DMA-BUF import on every Vulkan GPU (DMA-BUF on Linux only). Built
//! only with `--features hw-video`.
//!
//! Run with `VK_INSTANCE_LAYERS=VK_LAYER_KHRONOS_validation` and
//! `--nocapture` to see the validation layer's findings.
#![cfg(feature = "hw-video")]

use std::time::{Duration, Instant};

use twicecat_video::{
    ColorSpace, CpuFrame, Frame, PixelFormat, Plane, convert,
    gpu::{Converter, GpuDevice},
    probe::{self, Gpu},
    test_pattern::{self, TestPattern},
};

fn gpus() -> Vec<Gpu> {
    let found = probe::probe();
    assert!(
        !found.gpus.is_empty(),
        "no Vulkan GPU: {:?}",
        found.vulkan_error
    );
    found.gpus
}

fn bgra_gradient(w: u32, h: u32) -> CpuFrame {
    let mut data = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h {
        for x in 0..w {
            // A mix of smooth gradients and a hashed texture.
            let n = (x.wrapping_mul(73_856_093) ^ y.wrapping_mul(19_349_663)) >> 24;
            data.extend_from_slice(&[
                (x * 255 / w) as u8,
                (y * 255 / h) as u8,
                (n as u8) / 2 + ((x + y) % 128) as u8,
                255,
            ]);
        }
    }
    CpuFrame::new(
        PixelFormat::Bgra,
        w,
        h,
        vec![Plane {
            data: data.into(),
            stride: (w * 4) as usize,
        }],
        ColorSpace::default(),
        Duration::ZERO,
    )
    .expect("frame")
}

fn max_diff(a: &[u8], b: &[u8]) -> (u8, f64) {
    let max = a
        .iter()
        .zip(b)
        .map(|(x, y)| x.abs_diff(*y))
        .max()
        .unwrap_or(0);
    let mean = a
        .iter()
        .zip(b)
        .map(|(x, y)| f64::from(x.abs_diff(*y)))
        .sum::<f64>()
        / a.len() as f64;
    (max, mean)
}

#[test]
fn bgra_to_nv12_matches_the_cpu_converter_on_every_gpu() {
    let frame = bgra_gradient(1280, 720);
    let cpu = convert::to_nv12(&frame, ColorSpace::default()).expect("CPU conversion");
    for gpu in gpus() {
        let device = GpuDevice::open(Some(gpu.uuid)).expect("open device");
        let mut converter = Converter::new(&device).expect("converter");
        let out = converter
            .convert(
                &Frame::Cpu(frame.clone()),
                &[(1280, 720)],
                ColorSpace::default(),
            )
            .expect("GPU conversion");
        let (y_max, y_mean) = max_diff(&out[0].planes()[0].data, &cpu.planes()[0].data);
        let (c_max, c_mean) = max_diff(&out[0].planes()[1].data, &cpu.planes()[1].data);
        println!(
            "{}: luma max {y_max} mean {y_mean:.3}, chroma max {c_max} mean {c_mean:.3}",
            gpu.name
        );
        assert!(y_max <= 2 && y_mean < 0.6, "{}: luma off", gpu.name);
        assert!(c_max <= 3 && c_mean < 1.0, "{}: chroma off", gpu.name);
    }
}

#[test]
fn scaled_layers_keep_the_test_pattern_readable() {
    let pattern = TestPattern::new(3840, 2160);
    let frame = pattern.frame(1234, Duration::from_micros(987_654_321));
    for gpu in gpus() {
        let device = GpuDevice::open(Some(gpu.uuid)).expect("open device");
        let mut converter = Converter::new(&device).expect("converter");
        let sizes = [(3840, 2160), (1920, 1080), (960, 540)];
        let start = Instant::now();
        let out = converter
            .convert(&Frame::Cpu(frame.clone()), &sizes, ColorSpace::default())
            .expect("GPU scaling");
        let first = start.elapsed();
        let mut runs: Vec<Duration> = (0..20)
            .map(|_| {
                let start = Instant::now();
                converter
                    .convert(&Frame::Cpu(frame.clone()), &sizes, ColorSpace::default())
                    .expect("GPU scaling");
                start.elapsed()
            })
            .collect();
        runs.sort();
        println!(
            "{}: 4K to three layers with readback: first {first:?}, then median {:?}",
            gpu.name, runs[10]
        );
        // Same size: an exact copy.
        assert_eq!(
            out[0].planes()[0].data,
            frame.planes()[0].data,
            "{}",
            gpu.name
        );
        for (o, &(w, h)) in out.iter().zip(&sizes) {
            assert_eq!((o.width(), o.height()), (w, h));
            let stamp = test_pattern::read_stamp(o).expect("stamp readable");
            assert_eq!(stamp.counter, 1234, "{} at {w}x{h}", gpu.name);
        }
    }
}

#[cfg(target_os = "linux")]
#[test]
fn a_dma_buf_converts_like_the_same_frame_from_memory() {
    use twicecat_video::gpu::GpuError;

    let frame = bgra_gradient(1920, 1080);
    let devices: Vec<GpuDevice> = gpus()
        .iter()
        .map(|g| GpuDevice::open(Some(g.uuid)).expect("open device"))
        .filter(GpuDevice::can_import_dma_buf)
        .collect();
    assert!(!devices.is_empty(), "no GPU imports DMA-BUFs");
    // Each GPU imports its own buffers: conversion runs on the GPU that
    // owns the capture buffer (plan Task 4). Across GPUs frames go through
    // a CPU copy: the iGPU cannot reach a buffer in the RTX 5080's VRAM
    // (its submit fails with VK_ERROR_DEVICE_LOST, 2026-10-13).
    for exporter in &devices {
        for importer in devices.iter().filter(|d| d.uuid() == exporter.uuid()) {
            let dma = match twicecat_video::gpu::export_for_test(exporter, &frame) {
                Ok(d) => d,
                Err(GpuError::NotSupported(what)) => {
                    println!("{}: no linear export ({what})", exporter.name());
                    continue;
                }
                Err(e) => panic!("{}: export: {e}", exporter.name()),
            };
            let mut converter = Converter::new(importer).expect("converter");
            let from_memory = converter
                .convert(
                    &Frame::Cpu(frame.clone()),
                    &[(1920, 1080), (960, 540)],
                    ColorSpace::default(),
                )
                .expect("from memory");
            match converter.convert(
                &Frame::DmaBuf(dma),
                &[(1920, 1080), (960, 540)],
                ColorSpace::default(),
            ) {
                Ok(imported) => {
                    println!("{} -> {}: imported", exporter.name(), importer.name());
                    assert_eq!(imported, from_memory_with_ts(&from_memory, &imported));
                }
                Err(e) => panic!("{} -> {}: {e}", exporter.name(), importer.name()),
            }
        }
    }
}

/// The frames of `a` with the timestamps of `b` (the export keeps the
/// source's, the comparison is about pixels).
#[cfg(target_os = "linux")]
fn from_memory_with_ts(a: &[CpuFrame], b: &[CpuFrame]) -> Vec<CpuFrame> {
    a.iter()
        .zip(b)
        .map(|(x, y)| {
            CpuFrame::new(
                x.format(),
                x.width(),
                x.height(),
                x.planes().to_vec(),
                x.color(),
                y.timestamp(),
            )
            .expect("frame")
        })
        .collect()
}
