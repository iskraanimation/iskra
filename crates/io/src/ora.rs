//! OpenRaster files (`.ora`), which Krita, MyPaint and GIMP write: a ZIP archive with a list
//! of its layers, `stack.xml`, and a PNG for each. Read for a character to be brought in from
//! one as from a Photoshop file (plan §4.14).

use crate::character::{Layered, Node, Picture};
use crate::zip::Zip;

/// A layer of the file: the picture it is kept in, and where its corner is in the document.
struct Kept {
    src: String,
    left: i32,
    top: i32,
}

/// An OpenRaster file that was read: its size, its layers, and the tree they are in.
pub(crate) struct Ora {
    zip: Zip,
    pub(crate) width: u32,
    pub(crate) height: u32,
    layers: Vec<Kept>,
    tree: Vec<Tree>,
}

/// A layer or a group as the file lists it.
struct Tree {
    name: String,
    visible: bool,
    blend: crate::character::Blend,
    clipped: bool,
    layer: usize,
    children: Option<Vec<Tree>>,
}

impl Ora {
    pub(crate) fn open(bytes: Vec<u8>) -> Result<Ora, String> {
        let zip = Zip::open(bytes)?;
        let list = String::from_utf8(zip.read("stack.xml")?).map_err(|_| "its list of layers is not text")?;
        let list = roxmltree::Document::parse(&list).map_err(|why| format!("its list of layers could not be read: {why}"))?;
        let image = list.root_element();
        let side = |name: &str| image.attribute(name).and_then(|side| side.parse::<u32>().ok()).ok_or_else(|| format!("its list of layers has no `{name}`"));
        let (width, height) = (side("w")?, side("h")?);
        let stack = image.children().find(|child| child.has_tag_name("stack")).ok_or("its list has no stack of layers")?;
        let mut layers = Vec::new();
        let tree = Self::stack(stack, &mut layers);
        Ok(Ora { zip, width, height, layers, tree })
    }

    /// What a stack of the list holds, the bottom first: the list has the top first.
    fn stack(stack: roxmltree::Node, layers: &mut Vec<Kept>) -> Vec<Tree> {
        let number = |node: roxmltree::Node, name: &str| node.attribute(name).and_then(|at| at.parse::<f64>().ok()).map_or(0, |at| at.round() as i32);
        let mut held = Vec::new();
        for node in stack.children().filter(|node| node.is_element()) {
            let (name, visible) = (node.attribute("name").unwrap_or_default().to_string(), node.attribute("visibility") != Some("hidden"));
            let group = node.has_tag_name("stack");
            // A group is Pass Through unless it says it is apart, or names a mode.
            let op = node.attribute("composite-op").or((group && node.attribute("isolation") == Some("isolate")).then_some("svg:src-over"));
            let (blend, clipped) = crate::character::Blend::of_ora(op, group);
            if group {
                let children = Self::stack(node, layers);
                held.push(Tree { name, visible, blend, clipped, layer: 0, children: Some(children) });
            } else if let (true, Some(src)) = (node.has_tag_name("layer"), node.attribute("src")) {
                layers.push(Kept { src: src.to_string(), left: number(node, "x"), top: number(node, "y") });
                held.push(Tree { name, visible, blend, clipped, layer: layers.len() - 1, children: None });
            }
        }
        held.reverse();
        held
    }

    /// The tree of the file's layers, as the reader of a character walks it.
    pub(crate) fn nodes(&self) -> Vec<Node> {
        fn of(tree: &[Tree]) -> Vec<Node> {
            let node = |tree: &Tree| Node {
                name: tree.name.clone(),
                visible: tree.visible,
                blend: tree.blend.clone(),
                clipped: tree.clipped,
                layer: tree.layer,
                children: tree.children.as_deref().map(of),
            };
            tree.iter().map(node).collect()
        }
        of(&self.tree)
    }
}

impl Layered for Ora {
    fn pixels(&self, layer: usize) -> Result<Picture, String> {
        let kept = self.layers.get(layer).ok_or("the file has no such layer")?;
        let bytes = self.zip.read(&kept.src)?;
        let image = image::load_from_memory(&bytes).map_err(|why| format!("`{}` could not be read: {why}", kept.src))?.into_rgba8();
        Ok(Picture { left: kept.left, top: kept.top, width: image.width(), height: image.height(), rgba: image.into_raw() })
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use crate::{is_character, read_character};

    fn png(width: u32, height: u32, color: [u8; 4]) -> Vec<u8> {
        let mut bytes = Vec::new();
        let picture = image::RgbaImage::from_pixel(width, height, image::Rgba(color));
        picture.write_to(&mut std::io::Cursor::new(&mut bytes), image::ImageFormat::Png).unwrap();
        bytes
    }

    #[test]
    fn a_design_is_read_from_an_openraster_file_as_from_a_photoshop_one() {
        let dir = tempfile::tempdir().unwrap();
        let (red, green, blue, white) = ([255, 0, 0, 255], [0, 255, 0, 255], [0, 0, 255, 255], [255, 255, 255, 255]);
        // The list has the top of the stack first: notes to leave out, a slot of mouths of
        // which the second from the bottom is seen, a group of an arm with a merged hand,
        // and at the bottom a body.
        let list = r#"<?xml version="1.0" encoding="UTF-8"?>
<image version="0.0.3" w="20" h="10" xres="72" yres="72">
 <stack>
  <layer name="[ignore] notes" src="data/notes.png" x="0" y="0"/>
  <stack name="[slot] mouth">
   <layer name="O" src="data/o.png" x="6" y="7" visibility="visible" opacity="1.0"/>
   <layer name="A" src="data/a.png" x="6" y="6" visibility="hidden"/>
  </stack>
  <stack name="arm" x="0" y="0">
   <stack name="[merge] hand">
    <layer name="glove" src="data/glove.png" x="11" y="4" opacity="0.5"/>
    <layer name="palm" src="data/palm.png" x="10" y="4"/>
   </stack>
   <layer name="upper" src="data/upper.png" x="8" y="2"/>
  </stack>
  <layer name="body" src="data/body.png" x="4" y="2" composite-op="svg:src-over"/>
  <layer name="lost" src="data/lost.png" x="0" y="0"/>
 </stack>
</image>"#;
        let pictures = [("notes", white), ("o", green), ("a", red), ("glove", white), ("palm", blue), ("upper", green), ("body", red)];
        let pictures: Vec<(String, Vec<u8>)> = pictures.iter().map(|(name, color)| (format!("data/{name}.png"), png(2, 2, *color))).collect();
        let mut files: Vec<(&str, &[u8])> = vec![("mimetype", b"image/openraster"), ("stack.xml", list.as_bytes())];
        files.extend(pictures.iter().map(|(name, bytes)| (name.as_str(), &bytes[..])));
        let path = dir.path().join("hero.ora");
        std::fs::write(&path, crate::zip::stored(&files)).unwrap();

        let design = read_character(&path).unwrap();
        assert_eq!((design.width, design.height), (20, 10));
        // The bottom of the stack first; a layer whose picture is not in the file is said.
        let names: Vec<&str> = design.nodes.iter().map(|node| node.name.as_str()).collect();
        assert_eq!((names, &design.skipped[..]), (vec!["body", "upper", "hand", "mouth"], &["lost: it has no `data/lost.png`".to_string()][..]));
        let [body, upper, hand, mouth] = &design.nodes[..] else { panic!() };
        assert_eq!(((body.drawings[0].left, body.drawings[0].top), (upper.drawings[0].left, upper.drawings[0].top)), ((4, 2), (8, 2)));
        assert_eq!(&body.drawings[0].rgba[..4], &red[..]);
        // The glove whole over the palm, though it is at half its strength in the file.
        let merged = &hand.drawings[0];
        assert_eq!((merged.left, merged.top, merged.width, merged.height, &merged.rgba[..4], &merged.rgba[4..8]), (10, 4, 3, 2, &blue[..], &white[..]));
        // A slot's parts from the bottom, the hidden one too; the one that is seen is shown.
        assert_eq!((mouth.drawings.iter().map(|drawing| drawing.name.as_str()).collect::<Vec<_>>(), mouth.shown), (vec!["A", "O"], 1));

        // A layer laid `src-atop` is clipped to the one under it, and one laid `multiply` is
        // multiplied, in a merged group.
        let list = r#"<?xml version="1.0" encoding="UTF-8"?>
<image version="0.0.3" w="4" h="4">
 <stack>
  <stack name="[merge] arm">
   <layer name="shade" src="data/shade.png" x="0" y="0" composite-op="svg:src-atop"/>
   <layer name="dark" src="data/dark.png" x="0" y="0" composite-op="svg:multiply"/>
   <layer name="sleeve" src="data/sleeve.png" x="0" y="0"/>
  </stack>
 </stack>
</image>"#;
        let (shade, dark, sleeve) = (png(4, 4, red), png(2, 2, [128, 128, 128, 255]), png(2, 2, white));
        let files: Vec<(&str, &[u8])> = vec![
            ("mimetype", b"image/openraster"),
            ("stack.xml", list.as_bytes()),
            ("data/shade.png", &shade),
            ("data/dark.png", &dark),
            ("data/sleeve.png", &sleeve),
        ];
        let path = dir.path().join("clipped.ora");
        std::fs::write(&path, crate::zip::stored(&files)).unwrap();
        let arm = &read_character(&path).unwrap().nodes[0].drawings[0];
        assert_eq!((arm.width, &arm.rgba[..4]), (4, &red[..]), "the shade over the sleeve");
        assert_eq!(arm.rgba[(3 * 4 + 3) * 4 + 3], 0, "and not past it");

        // What is no OpenRaster file says so.
        let path = dir.path().join("broken.ora");
        std::fs::write(&path, crate::zip::stored(&[("mimetype", b"image/openraster")])).unwrap();
        assert!(read_character(&path).is_err_and(|why| why.contains("no `stack.xml`")));
        std::fs::write(&path, b"only text").unwrap();
        assert!(read_character(&path).is_err_and(|why| why.contains("not a ZIP archive")));
        assert!(is_character(&path) && is_character(Path::new("A.ORA")) && !is_character(Path::new("a.kra")));
    }
}
