//! `export.png` renderer (G-019): a fresh cushion-shaded treemap of a subtree at a requested size.
//!
//! The engine owns the command, the path checks and the atomic write; this module only turns a
//! tree into PNG bytes, because the colours (`theme::Tokens`) and the cushion renderer
//! (`cushion_view::render`) live in the UI layer. It is registered once per process with
//! `dirustat_engine::export::register_png_renderer` when the app is created.

use crate::cushion_view::{CushionInput, CushionTile, background_rgb, render};
use crate::map_panel::tile_color;
use crate::screenshot::encode_png;
use crate::theme::{ThemeKind, Tokens};
use dirustat_engine::MapLayout;
use dirustat_engine::export::PngRequest;
use dirustat_model::{NodeId, Tree};
use dirustat_treemap::{CushionConfig, LayoutOptions, Rect};

/// Registers [`render_png`] as the process-wide renderer (a no-op after the first call).
pub fn register() {
    let _ = dirustat_engine::export::register_png_renderer(render_png);
}

/// Lays out the subtree of `request.root` in `width x height` pixels, shades it and encodes a PNG.
/// Pure and deterministic for the same tree and request.
pub fn render_png(tree: &Tree, request: &PngRequest) -> Result<Vec<u8>, String> {
    let (width, height) = (usize::try_from(request.width), usize::try_from(request.height));
    let (Ok(w), Ok(h)) = (width, height) else { return Err("image size out of range".to_owned()) };
    if w == 0 || h == 0 {
        return Err("image size out of range".to_owned());
    }
    #[allow(clippy::cast_precision_loss)] // the sides are validated to be at most 8192
    let bounds = Rect { x: 0.0, y: 0.0, w: request.width as f32, h: request.height as f32 };
    let tokens = Tokens::for_kind(if request.dark { ThemeKind::Dark } else { ThemeKind::Light });
    let layout = MapLayout::build_from(tree, request.root, bounds, request.mode, LayoutOptions::default());
    let tiles = layout
        .tiles()
        .iter()
        .map(|t| {
            let (kind, name) = tree.get(t.node).map_or((dirustat_model::NodeKind::Dir, ""), |n| (n.kind(), n.name()));
            let [r, g, b, _] = tile_color(kind, name, &tokens, false);
            CushionTile {
                rect: t.rect,
                color: [r, g, b],
                node: t.node.index(),
                parent: parent_in_view(tree, t.node, request.root),
                leaf: t.leaf,
                remainder: t.remainder,
            }
        })
        .collect();
    let input = CushionInput {
        size: [w, h],
        scale: 1.0,
        background: background_rgb(&tokens),
        tiles,
        config: CushionConfig::default(),
    };
    let image = render(&input);
    encode_png(&image, None, 1.0).map(|(bytes, _, _)| bytes)
}

/// The parent of `node` as the layout sees it: the subtree root has none.
fn parent_in_view(tree: &Tree, node: NodeId, root: NodeId) -> Option<usize> {
    if node == root { None } else { tree.get(node).and_then(|n| n.parent()).map(NodeId::index) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dirustat_engine::Session;
    use dirustat_model::{NodeKind, SizeMode};
    use serde_json::json;

    fn tree() -> Tree {
        let mut t = Tree::new("root");
        let docs = t.add_child(t.root(), "Docs", NodeKind::Dir, 0).unwrap();
        t.add_child(docs, "a.txt", NodeKind::File, 5000).unwrap();
        t.add_child(docs, "b.png", NodeKind::File, 3000).unwrap();
        t.add_child(t.root(), "big.bin", NodeKind::File, 9000).unwrap();
        t.add_child(t.root(), "small.zip", NodeKind::File, 100).unwrap();
        t.aggregate();
        t
    }

    fn decode(bytes: &[u8]) -> (png::OutputInfo, Vec<u8>) {
        let mut reader = png::Decoder::new(std::io::Cursor::new(bytes)).read_info().unwrap();
        let mut buf = vec![0; reader.output_buffer_size().unwrap()];
        let info = reader.next_frame(&mut buf).unwrap();
        (info, buf)
    }

    fn request(root: NodeId, dark: bool) -> PngRequest {
        PngRequest { root, width: 320, height: 200, mode: SizeMode::Logical, dark }
    }

    #[test]
    fn png_decodes_with_the_requested_size_and_is_shaded() {
        let t = tree();
        let bytes = render_png(&t, &request(t.root(), true)).unwrap();
        let (info, pixels) = decode(&bytes);
        assert_eq!((info.width, info.height), (320, 200));
        let distinct: std::collections::HashSet<&[u8]> = pixels.chunks(4).collect();
        assert!(distinct.len() > 8, "cushion shading gives many shades, got {}", distinct.len());
        assert_eq!(bytes, render_png(&t, &request(t.root(), true)).unwrap(), "deterministic");
        // The theme only colours the background and directories, which the tiles cover completely.
        assert!(render_png(&t, &request(t.root(), false)).is_ok());
    }

    #[test]
    fn a_subtree_can_be_drawn() {
        let t = tree();
        let docs = t.children(t.root()).next().unwrap();
        let whole = render_png(&t, &request(t.root(), true)).unwrap();
        let sub = render_png(&t, &request(docs, true)).unwrap();
        assert_ne!(whole, sub);
        assert_eq!(decode(&sub).0.width, 320);
    }

    #[test]
    fn hostile_sizes_are_errors_not_panics() {
        let t = tree();
        for (w, h) in [(0, 10), (10, 0), (1, 1), (16, 16)] {
            let req = PngRequest { width: w, height: h, ..request(t.root(), true) };
            let _ = render_png(&t, &req);
        }
        let empty = Tree::new("x");
        assert!(render_png(&empty, &request(empty.root(), true)).is_ok());
    }

    #[test]
    fn the_command_writes_a_png_through_the_registered_renderer() {
        register();
        let dir = tempfile::tempdir().unwrap();
        let scan = dir.path().join("scan");
        std::fs::create_dir(&scan).unwrap();
        std::fs::write(scan.join("a.bin"), vec![1_u8; 4000]).unwrap();
        std::fs::write(scan.join("b.txt"), vec![1_u8; 1000]).unwrap();
        let mut s = Session::new();
        let handle = dirustat_scan::start(&scan, dirustat_scan::ScanOptions::default()).unwrap();
        assert_eq!(s.finish_scan(handle), Ok(true));
        let out = dir.path().join("map.png");
        let params = json!({"path": out.to_string_lossy(), "width": 200, "height": 120, "theme": "light", "allow_inside_scan": false});
        let err = s.execute("export.png", &json!({"path": scan.join("m.png").to_string_lossy()})).unwrap_err();
        assert_eq!(err.code(), "invalid_params", "inside the scanned folder: {err}");
        let done = s.execute("export.png", &params).unwrap();
        assert_eq!((done["width"].as_u64(), done["height"].as_u64()), (Some(200), Some(120)));
        let (info, _) = decode(&std::fs::read(&out).unwrap());
        assert_eq!((info.width, info.height), (200, 120));
        let again = s.execute("export.png", &params).unwrap_err();
        assert!(again.to_string().contains("overwrite"), "{again}");
        let params = json!({"path": out.to_string_lossy(), "width": 64, "height": 64, "overwrite": true});
        s.execute("export.png", &params).unwrap();
        assert_eq!(decode(&std::fs::read(&out).unwrap()).0.width, 64);
        assert!(
            std::fs::read_dir(dir.path()).unwrap().all(|e| !e.unwrap().file_name().to_string_lossy().ends_with(".tmp"))
        );
        for bad in [json!({"width": 0}), json!({"width": 9000}), json!({"width": 8192, "height": 8192})] {
            let mut p = params.clone();
            p.as_object_mut().unwrap().extend(bad.as_object().unwrap().clone());
            assert_eq!(s.execute("export.png", &p).unwrap_err().code(), "invalid_params");
        }
    }
}
