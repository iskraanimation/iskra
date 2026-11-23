//! Workspace automation: `cargo xtask <layers|ci>`.
//!
//! - `layers`: enforces the dependency layering (plan §3): a crate depends only on lower
//!   layers, and nothing below the UI depends on a UI toolkit, windowing or OS audio crate.
//! - `ci`: what CI runs: format check, clippy with warnings as errors, tests, layers.
//! - `icon`: the app's icon, `apps/iskra/assets/iskra.svg`, drawn as the PNG the window is
//!   given and as the ICO that Windows puts into `iskra.exe`. Both are kept in the repository,
//!   so that a build needs neither this nor an SVG renderer.

use std::process::{Command, ExitCode};

use serde_json::Value;

/// (package name, layer). A crate may depend on workspace crates of a lower layer only.
const LAYERS: &[(&str, u8)] = &[
    ("iskra-photocraft-geom", 0),
    ("iskra-photocraft-cms", 0),
    ("iskra-photocraft-color", 1),
    ("iskra-photocraft-raster", 2),
    ("iskra-photocraft-paint", 3),
    ("iskra-photocraft-tablet", 0),
    ("iskra-tablet-windows", 0),
    ("iskra-photocraft-psd", 0),
    ("iskra-effectcraft-time", 0),
    ("iskra-effectcraft-geom", 0),
    ("iskra-effectcraft-color", 0),
    ("iskra-effectcraft-keyframe", 1),
    ("iskra-effectcraft-raster", 1),
    ("iskra-filmcraft-bitstream", 0),
    ("iskra-filmcraft-testkit", 0),
    ("iskra-filmcraft-h264enc", 1),
    ("iskra-filmcraft-prores", 1),
    ("iskra-filmcraft-aac", 1),
    ("iskra-filmcraft-isobmff", 1),
    ("iskra-vectorcraft-geom", 0),
    ("iskra-vectorcraft-pathops", 1),
    ("iskra-vectorcraft-trace", 2),
    ("iskra-audio", 0),
    ("iskra-drawing", 4),
    ("iskra-model", 5),
    ("iskra-eval", 6),
    ("iskra-store", 6),
    ("iskra-io", 6),
    ("iskra-render", 7),
    ("iskra-gpu", 8),
    ("iskra-export", 8),
    ("iskra-engine", 9),
    ("iskra-script", 10),
    ("iskra-ui", 11),
    ("iskra", 12),
    ("iskra-extract", 12),
    ("iskra-render-cli", 12),
];

/// The UI layer: crates below it must not depend on [`UI_ONLY`].
const UI_LAYER: u8 = 11;

/// UI toolkits, windowing and OS audio.
const UI_ONLY: &[&str] = &["egui", "egui_extras", "eframe", "egui-wgpu", "winit", "rfd", "cpal"];

fn layer_of(name: &str) -> Option<u8> {
    LAYERS.iter().find(|(n, _)| *n == name).map(|(_, l)| *l)
}

/// Each workspace package with its normal and build dependencies (dev-dependencies don't ship).
fn workspace_crates() -> Result<Vec<(String, Vec<String>)>, String> {
    let out = Command::new(env!("CARGO")).args(["metadata", "--format-version", "1", "--no-deps"]).output().map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).into());
    }
    let metadata: Value = serde_json::from_slice(&out.stdout).map_err(|e| e.to_string())?;
    let packages = metadata["packages"].as_array().ok_or("cargo metadata: no packages")?;
    Ok(packages
        .iter()
        .filter(|p| p["name"] != "xtask")
        .map(|p| {
            let deps = p["dependencies"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|d| d["kind"].is_null() || d["kind"] == "build")
                .filter_map(|d| d["name"].as_str().map(str::to_string))
                .collect();
            (p["name"].as_str().unwrap_or_default().to_string(), deps)
        })
        .collect())
}

fn layers() -> Result<(), String> {
    let mut errors = Vec::new();
    for (name, deps) in workspace_crates()? {
        let Some(layer) = layer_of(&name) else {
            errors.push(format!("{name}: not assigned a layer (add it to LAYERS in xtask/src/main.rs)"));
            continue;
        };
        for dep in &deps {
            if layer < UI_LAYER && UI_ONLY.contains(&dep.as_str()) {
                errors.push(format!("{name} (L{layer}) depends on the UI/OS crate `{dep}`"));
            }
            if let Some(dep_layer) = layer_of(dep)
                && dep_layer >= layer
            {
                errors.push(format!("{name} (L{layer}) depends on {dep} (L{dep_layer}), which is not a lower layer"));
            }
        }
    }
    if errors.is_empty() {
        println!("layers: ok");
        Ok(())
    } else {
        Err(errors.join("\n"))
    }
}

fn cargo(args: &[&str]) -> Result<(), String> {
    println!("cargo {}", args.join(" "));
    let status = Command::new(env!("CARGO")).args(args).status().map_err(|e| e.to_string())?;
    if status.success() { Ok(()) } else { Err(format!("cargo {} failed", args.join(" "))) }
}

fn ci() -> Result<(), String> {
    cargo(&["fmt", "--all", "--", "--check"])?;
    cargo(&["clippy", "--workspace", "--all-targets", "--locked", "--", "-D", "warnings"])?;
    cargo(&["test", "--workspace", "--locked"])?;
    layers()
}

/// The sizes Windows asks an ICO for: Explorer's lists, the taskbar, and its largest view.
const ICO_SIZES: [u32; 7] = [16, 24, 32, 48, 64, 128, 256];

fn icon() -> Result<(), String> {
    let assets = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../apps/iskra/assets");
    let svg = std::fs::read(assets.join("iskra.svg")).map_err(|e| format!("iskra.svg: {e}"))?;
    let tree = resvg::usvg::Tree::from_data(&svg, &resvg::usvg::Options::default()).map_err(|e| format!("iskra.svg: {e}"))?;
    let png = |side: u32| -> Result<Vec<u8>, String> {
        let mut pixmap = resvg::tiny_skia::Pixmap::new(side, side).ok_or("no picture of that size")?;
        let scale = side as f32 / tree.size().width();
        resvg::render(&tree, resvg::tiny_skia::Transform::from_scale(scale, scale), &mut pixmap.as_mut());
        // tiny-skia's pixels are premultiplied; a PNG's are not.
        let straight: Vec<u8> = pixmap
            .pixels()
            .iter()
            .flat_map(|p| {
                let c = p.demultiply();
                [c.red(), c.green(), c.blue(), c.alpha()]
            })
            .collect();
        let image = image::RgbaImage::from_raw(side, side, straight).ok_or("the picture's size")?;
        let mut bytes = Vec::new();
        image.write_to(&mut std::io::Cursor::new(&mut bytes), image::ImageFormat::Png).map_err(|e| e.to_string())?;
        Ok(bytes)
    };
    std::fs::write(assets.join("iskra.png"), png(1024)?).map_err(|e| e.to_string())?;
    // An ICO of PNGs: a header, an entry for each size, then the pictures.
    let pictures: Vec<Vec<u8>> = ICO_SIZES.iter().map(|side| png(*side)).collect::<Result<_, _>>()?;
    let mut ico = vec![0, 0, 1, 0];
    ico.extend((ICO_SIZES.len() as u16).to_le_bytes());
    let mut offset = 6 + 16 * ICO_SIZES.len() as u32;
    for (side, picture) in ICO_SIZES.iter().zip(&pictures) {
        // 0 stands for 256.
        let edge = if *side >= 256 { 0 } else { *side as u8 };
        ico.extend([edge, edge, 0, 0]);
        ico.extend(1u16.to_le_bytes());
        ico.extend(32u16.to_le_bytes());
        ico.extend((picture.len() as u32).to_le_bytes());
        ico.extend(offset.to_le_bytes());
        offset += picture.len() as u32;
    }
    pictures.iter().for_each(|picture| ico.extend(picture));
    std::fs::write(assets.join("iskra.ico"), ico).map_err(|e| e.to_string())?;
    println!("icon: apps/iskra/assets/iskra.png and iskra.ico made");
    Ok(())
}

fn main() -> ExitCode {
    let result = match std::env::args().nth(1).as_deref() {
        Some("layers") => layers(),
        Some("ci") => ci(),
        Some("icon") => icon(),
        _ => Err("usage: cargo xtask <layers|ci|icon>".into()),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
}
