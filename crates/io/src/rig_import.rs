//! Rig import (plan §4.17): a rig brought from the scene of another animation program, as near
//! as it goes. This part reads the scene file, which is XML, into what it says: the drawings
//! it has (elements, each a folder of drawing files), its nodes in their groups with their
//! attributes, the links between them, and the columns that give a value on each frame.
//!
//! Only the rig is wanted, not its animation: a value is read as it is on the scene's first
//! frame, and a drawing node shows the drawing it shows there. Turning this into a shot is the
//! engine's; the drawings' pictures come from the converter the user points at.

use std::collections::HashMap;
use std::ops::RangeInclusive;
use std::path::{Path, PathBuf};

/// A scene, as its file says.
#[derive(Clone, Debug, PartialEq)]
pub struct RigScene {
    /// Where the scene file is: the elements' folders are under it.
    pub folder: PathBuf,
    /// The camera's size in pixels.
    pub camera: [u32; 2],
    pub frame_rate: f64,
    /// The scene's grid: how many units it is across, up and in depth.
    pub units: [f64; 3],
    /// The shape of one unit of the grid, across by up.
    pub unit_aspect: [f64; 2],
    /// The frame the rig is read at.
    pub first_frame: u32,
    pub elements: Vec<Element>,
    /// The scene's top group, with every node in it and in the groups under it.
    pub top: Group,
    columns: HashMap<String, Column>,
}

/// An element: a folder of drawings, which drawing nodes show.
#[derive(Clone, Debug, PartialEq)]
pub struct Element {
    pub id: u32,
    pub name: String,
    /// The folder of its drawings, under the scene's folder.
    pub folder: PathBuf,
    /// The names of its drawings, as the file lists them.
    pub drawings: Vec<String>,
    /// How many of the scene's fields its drawings' 12 are: at 12, a drawing is the size it was
    /// drawn at.
    pub field_chart: f64,
}

impl Element {
    /// The file of one of its drawings: `<folder>/<element>-<drawing>.tvg`.
    pub fn drawing_file(&self, scene: &RigScene, drawing: &str) -> PathBuf {
        scene.folder.join(&self.folder).join(format!("{}-{drawing}.tvg", self.name))
    }
}

/// A group of nodes, and the links between them. A link names nodes of this group, or the
/// group's own ports, by name.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Group {
    pub name: String,
    pub nodes: Vec<RigNode>,
    pub links: Vec<Link>,
    /// The boxes drawn round some of its nodes in the node view, to show them as belonging
    /// together.
    pub backdrops: Vec<Backdrop>,
}

/// A box drawn round nodes in the node view, with a title: which nodes it holds is where they
/// are, inside it, as the file keeps no list of them.
#[derive(Clone, Debug, PartialEq)]
pub struct Backdrop {
    pub title: String,
    /// Its corners in the node view, the left and top ones first.
    pub rect: [f64; 4],
}

impl Backdrop {
    pub fn contains(&self, [x, y]: [f64; 2]) -> bool {
        let [x0, y0, x1, y1] = self.rect;
        (x0.min(x1)..=x0.max(x1)).contains(&x) && (y0.min(y1)..=y0.max(y1)).contains(&y)
    }

    pub fn middle(&self) -> [f64; 2] {
        let [x0, y0, x1, y1] = self.rect;
        [(x0 + x1) / 2.0, (y0 + y1) / 2.0]
    }

    pub fn area(&self) -> f64 {
        let [x0, y0, x1, y1] = self.rect;
        ((x1 - x0) * (y1 - y0)).abs()
    }
}

/// A node: its kind (`PEG`, `READ`, `COMPOSITE`, …, or `GROUP`), its name, and its attributes.
#[derive(Clone, Debug, PartialEq)]
pub struct RigNode {
    pub kind: String,
    pub name: String,
    pub attrs: Attr,
    /// What is in it, for a group.
    pub group: Option<Group>,
    /// Where it is in the node view of its group.
    pub pos: Option<[f64; 2]>,
}

/// An attribute: a value, the column that animates it, and the attributes under it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Attr {
    pub value: Option<String>,
    pub column: Option<String>,
    pub children: Vec<(String, Attr)>,
}

impl Attr {
    /// The attribute at a path of names joined by dots, such as `position.x`.
    pub fn get(&self, path: &str) -> Option<&Attr> {
        path.split('.').try_fold(self, |attr, name| attr.children.iter().find(|(named, _)| named == name).map(|(_, attr)| attr))
    }

    /// The attribute's value at `path` as a number, not looking at its column.
    pub fn number(&self, path: &str) -> Option<f64> {
        self.get(path)?.value.as_deref()?.parse().ok()
    }

    /// The attribute's value at `path` as a switch.
    pub fn switch(&self, path: &str) -> Option<bool> {
        match self.get(path)?.value.as_deref()? {
            "true" | "Y" => Some(true),
            "false" | "N" => Some(false),
            _ => None,
        }
    }
}

/// A link from an output of `from` to an input of `to`; the ports, when the file gives them.
/// With no input port, a link takes the next of `to`'s ports; with no output port, the first.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Link {
    pub from: String,
    pub to: String,
    pub port: Option<u32>,
    pub out_port: Option<u32>,
}

/// A node with the groups looked through: what is wired into each of its input ports, by the
/// index of the node it comes from in the same list, the port's number first.
#[derive(Clone, Debug, PartialEq)]
pub struct Wired<'a> {
    /// The groups it is in, the outermost first.
    pub path: Vec<&'a str>,
    pub node: &'a RigNode,
    pub inputs: Vec<(u32, usize)>,
}

/// The kinds of node that are a group's ports, which the groups being looked through take away.
const PORT_IN: &str = "MULTIPORT_IN";
const PORT_OUT: &str = "MULTIPORT_OUT";

/// A key of a column of values: a value from its frame on, held there or eased to the next.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Key {
    pub frame: u32,
    pub value: f64,
    pub held: bool,
}

#[derive(Clone, Debug, PartialEq)]
enum Column {
    /// Which drawing of an element is shown on which frames.
    Drawings { element: u32, shown: Vec<(String, Vec<RangeInclusive<u32>>)> },
    /// A value over time, by its keys, in the order of their frames.
    Keys(Vec<Key>),
    /// A position over time, by points on a path in three dimensions.
    Path(Vec<(u32, [f64; 3])>),
    /// A column of another kind, which the rig doesn't need.
    Other,
}

/// The kind given to a group, which the file writes apart from its other nodes.
pub const GROUP: &str = "GROUP";

impl RigScene {
    /// Read a scene file.
    pub fn read(path: &Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(path).map_err(|why| format!("The scene could not be read: {why}"))?;
        Self::parse(&text, path.parent().unwrap_or(Path::new("")))
    }

    /// Read a scene from its text; `folder` is where its elements' folders are.
    pub fn parse(text: &str, folder: &Path) -> Result<Self, String> {
        let document = roxmltree::Document::parse(text).map_err(|why| format!("The scene is not a file this import reads: {why}"))?;
        let project = document.root_element();
        let numbers = |node: Option<roxmltree::Node>, name: &str| -> Vec<f64> {
            node.and_then(|node| node.attribute(name)).map_or(Vec::new(), |at| at.split(',').filter_map(|part| part.trim().parse().ok()).collect())
        };
        let options = child(project, "options");
        let resolution = options.and_then(|options| child(options, "resolution"));
        let size = numbers(resolution, "size");
        let camera = match size[..] {
            [width, height] if width > 0.0 && height > 0.0 => [width as u32, height as u32],
            _ => return Err("The scene has no camera size.".to_string()),
        };
        let frame_rate = numbers(options.and_then(|options| child(options, "framerate")), "val").first().copied().unwrap_or(24.0);
        let metrics = options.and_then(|options| child(options, "metrics"));
        let metric = |name: &str, or: f64| numbers(metrics, name).first().copied().unwrap_or(or);
        let units = [metric("numberOfUnitsX", 24.0), metric("numberOfUnitsY", 24.0), metric("numberOfUnitsZ", 12.0)];
        let unit_aspect = [metric("unitAspectRatioX", 4.0), metric("unitAspectRatioY", 3.0)];

        let mut elements = Vec::new();
        for element in child(project, "elements").into_iter().flat_map(|list| list.children().filter(|child| child.has_tag_name("element"))) {
            let Some(id) = element.attribute("id").and_then(|id| id.parse().ok()) else { continue };
            let name = element.attribute("elementName").unwrap_or_default().to_string();
            let root = element.attribute("rootFolder").unwrap_or("elements");
            let folder = Path::new(root).join(element.attribute("elementFolder").unwrap_or(&name));
            let drawings = child(element, "drawings")
                .into_iter()
                .flat_map(|list| list.children().filter(|child| child.has_tag_name("dwg")))
                .filter_map(|drawing| drawing.attribute("name").map(str::to_string))
                .collect();
            let field_chart = element.attribute("fieldChart").and_then(|chart| chart.parse().ok()).filter(|chart: &f64| *chart > 0.0).unwrap_or(12.0);
            elements.push(Element { id, name, folder, drawings, field_chart });
        }

        // The scene the file opens on: the first, which has the top group.
        let scene = child(project, "scenes")
            .and_then(|scenes| scenes.children().find(|scene| scene.has_tag_name("scene")))
            .ok_or("The scene file has no scene in it.")?;
        let first_frame = scene.attribute("startFrame").and_then(|frame| frame.parse().ok()).unwrap_or(1);
        let mut columns = HashMap::new();
        for column in child(scene, "columns").into_iter().flat_map(|list| list.children().filter(|child| child.has_tag_name("column"))) {
            let Some(name) = column.attribute("name") else { continue };
            columns.insert(name.to_string(), read_column(column));
        }
        let top = child(scene, "rootgroup").map(read_group).ok_or("The scene has no nodes.")?;
        Ok(Self { folder: folder.to_path_buf(), camera, frame_rate, units, unit_aspect, first_frame, elements, top, columns })
    }

    /// Every node, the groups' too, each with the names of the groups it is in.
    pub fn nodes(&self) -> Vec<(Vec<&str>, &RigNode)> {
        fn walk<'a>(group: &'a Group, path: &mut Vec<&'a str>, all: &mut Vec<(Vec<&'a str>, &'a RigNode)>) {
            for node in &group.nodes {
                all.push((path.clone(), node));
                if let Some(inner) = &node.group {
                    path.push(&node.name);
                    walk(inner, path, all);
                    path.pop();
                }
            }
        }
        let mut all = Vec::new();
        walk(&self.top, &mut Vec::new(), &mut all);
        all
    }

    /// The scene's palette files, in the order its list of palettes gives them: each line of the
    /// list names a palette by its path in the scene's folder, without `.plt`. With no list,
    /// every palette in the folder `palette-library`, by name.
    pub fn palettes(&self) -> Vec<PathBuf> {
        let listed = std::fs::read_to_string(self.folder.join("PALETTE_LIST")).ok().map(|list| palette_list(&list, &self.folder));
        if let Some(listed) = listed.filter(|listed| !listed.is_empty()) {
            return listed;
        }
        let mut found: Vec<PathBuf> = std::fs::read_dir(self.folder.join("palette-library"))
            .into_iter()
            .flatten()
            .filter_map(|entry| Some(entry.ok()?.path()))
            .filter(|path| path.extension().is_some_and(|ending| ending.eq_ignore_ascii_case("plt")))
            .collect();
        found.sort();
        found
    }

    /// The group at `path`, from the top group down by the names of the groups in it.
    fn group_at(&self, path: &[&str]) -> Option<&Group> {
        path.iter().try_fold(&self.top, |group, name| group.nodes.iter().find(|node| node.name == *name)?.group.as_ref())
    }

    /// Every backdrop of the scene, with the path of the group it is in, the outermost first.
    pub fn backdrops(&self) -> Vec<(Vec<&str>, &Backdrop)> {
        fn collect<'a>(group: &'a Group, path: &mut Vec<&'a str>, all: &mut Vec<(Vec<&'a str>, &'a Backdrop)>) {
            all.extend(group.backdrops.iter().map(|backdrop| (path.clone(), backdrop)));
            for node in &group.nodes {
                if let Some(inner) = &node.group {
                    path.push(&node.name);
                    collect(inner, path, all);
                    path.pop();
                }
            }
        }
        let mut all = Vec::new();
        collect(&self.top, &mut Vec::new(), &mut all);
        all
    }

    /// Where something in the group at `path`, or in a group inside it, is in that group's node
    /// view: something of it where it is, at `at`, one further in where the group it is in is
    /// there. `inside` is the path of the groups it is in, as [`Wired::path`] has it.
    pub fn place(&self, path: &[&str], inside: &[&str], at: Option<[f64; 2]>) -> Option<[f64; 2]> {
        let rest = inside.strip_prefix(path)?;
        match rest.first() {
            None => at,
            Some(name) => self.group_at(path)?.nodes.iter().find(|node| node.name == *name)?.pos,
        }
    }

    /// Every node but the groups and their ports, with what is wired into it: a link into a
    /// group goes on to what the group's in port is linked to inside it, and a link out of a
    /// group comes from what is linked to its out port.
    pub fn wired(&self) -> Vec<Wired<'_>> {
        /// The groups from the top down to the one a link is in, each with its name.
        type Stack<'a> = Vec<(&'a str, &'a Group)>;
        fn collect<'a>(stack: &mut Stack<'a>, all: &mut Vec<Wired<'a>>) {
            let group = stack.last().expect("a group").1;
            for node in &group.nodes {
                match &node.group {
                    Some(inner) => {
                        stack.push((&node.name, inner));
                        collect(stack, all);
                        stack.pop();
                    }
                    None if node.kind == PORT_IN || node.kind == PORT_OUT => {}
                    None => all.push(Wired { path: stack[1..].iter().map(|(name, _)| *name).collect(), node, inputs: Vec::new() }),
                }
            }
        }
        /// Each link of the group with the input port it takes.
        fn ported(group: &Group) -> Vec<(u32, &Link)> {
            let mut next: HashMap<&str, u32> = HashMap::new();
            group
                .links
                .iter()
                .map(|link| {
                    let counted = next.entry(&link.to).or_default();
                    let port = link.port.unwrap_or(*counted);
                    *counted = port + 1;
                    (port, link)
                })
                .collect()
        }
        /// The nodes an output of `name`, in the group at the top of `stack`, comes from.
        fn sources<'a>(stack: &[(&'a str, &'a Group)], name: &str, out_port: u32, at: &dyn Fn(&[&str], &str) -> Option<usize>, depth: usize) -> Vec<usize> {
            let Some(&(_, group)) = stack.last() else { return Vec::new() };
            let Some(node) = group.nodes.iter().find(|node| node.name == name).filter(|_| depth < 64) else { return Vec::new() };
            match &node.group {
                // Out of a group: from what is linked to its out port, inside it.
                Some(inner) => {
                    let mut inside = stack.to_vec();
                    inside.push((&node.name, inner));
                    let ports: Vec<&str> = inner.nodes.iter().filter(|node| node.kind == PORT_OUT).map(|node| node.name.as_str()).collect();
                    ported(inner)
                        .into_iter()
                        .filter(|(port, link)| *port == out_port && ports.contains(&link.to.as_str()))
                        .flat_map(|(_, link)| sources(&inside, &link.from, link.out_port.unwrap_or(0), at, depth + 1))
                        .collect()
                }
                // A group's in port: from what is linked to the group, outside it.
                None if node.kind == PORT_IN => {
                    let Some(((own, _), outside)) = stack.split_last() else { return Vec::new() };
                    let Some(&(_, around)) = outside.last() else { return Vec::new() };
                    ported(around)
                        .into_iter()
                        .filter(|(port, link)| *port == out_port && link.to == *own)
                        .flat_map(|(_, link)| sources(outside, &link.from, link.out_port.unwrap_or(0), at, depth + 1))
                        .collect()
                }
                None => {
                    let path: Vec<&str> = stack[1..].iter().map(|(name, _)| *name).collect();
                    at(&path, name).into_iter().collect()
                }
            }
        }
        fn link<'a>(stack: &mut Stack<'a>, all: &mut Vec<Wired<'a>>, index: &HashMap<(Vec<&'a str>, &'a str), usize>) {
            let group = stack.last().expect("a group").1;
            let path: Vec<&str> = stack[1..].iter().map(|(name, _)| *name).collect();
            let at = |path: &[&str], name: &str| index.get(&(path.to_vec(), name)).copied();
            for (port, link) in ported(group) {
                // Into a node of this group; links into a group or a port are followed from
                // the other end.
                let Some(to) = at(&path, &link.to) else { continue };
                for from in sources(stack, &link.from, link.out_port.unwrap_or(0), &at, 0) {
                    all[to].inputs.push((port, from));
                }
            }
            for node in &group.nodes {
                if let Some(inner) = &node.group {
                    stack.push((&node.name, inner));
                    link(stack, all, index);
                    stack.pop();
                }
            }
        }
        let mut all = Vec::new();
        let mut stack: Stack = vec![(self.top.name.as_str(), &self.top)];
        collect(&mut stack, &mut all);
        let index: HashMap<(Vec<&str>, &str), usize> = all.iter().enumerate().map(|(at, wired)| ((wired.path.clone(), wired.node.name.as_str()), at)).collect();
        link(&mut stack, &mut all, &index);
        for wired in &mut all {
            wired.inputs.sort_by_key(|(port, _)| *port);
        }
        all
    }

    /// How many scene units, a shot's pixels at depth 0, one unit of the scene's grid is, across,
    /// up and in depth, in a shot `height` pixels high. The grid's height is the frame's, as
    /// the scene's camera fits it; a unit across is as much wider as the grid's units are;
    /// depth is in units across.
    pub fn unit_size(&self, height: u32) -> [f64; 3] {
        let up = f64::from(height) / self.units[1].max(1e-6);
        let across = up * self.unit_aspect[0] / self.unit_aspect[1].max(1e-6);
        [across, up, across]
    }

    pub fn element(&self, id: u32) -> Option<&Element> {
        self.elements.iter().find(|element| element.id == id)
    }

    /// A number of a node at `path`, as it is on the first frame: from the column that animates
    /// it, or else its value.
    pub fn number(&self, node: &RigNode, path: &str) -> Option<f64> {
        let attr = node.attrs.get(path)?;
        if let Some(Column::Keys(keys)) = attr.column.as_ref().and_then(|column| self.columns.get(column)) {
            return value_at(keys, self.first_frame);
        }
        attr.value.as_deref()?.parse().ok()
    }

    /// Where a node is on the first frame, x, y and z: its own three values, or the point of its
    /// path when it follows one.
    pub fn position(&self, node: &RigNode) -> [f64; 3] {
        let path = node.attrs.switch("position.separate") == Some(false);
        let on_path = node.attrs.get("position.attr3dpath").and_then(|attr| attr.column.as_ref()).and_then(|column| self.columns.get(column));
        if let (true, Some(Column::Path(points))) = (path, on_path) {
            let point = points.iter().rev().find(|(frame, _)| *frame <= self.first_frame).or(points.first());
            return point.map_or([0.0; 3], |(_, at)| *at);
        }
        ["x", "y", "z"].map(|axis| self.number(node, &format!("position.{axis}")).unwrap_or(0.0))
    }

    /// The element a drawing node shows, and the name of the drawing it shows on the first frame
    /// (none where it shows none there).
    pub fn drawing(&self, node: &RigNode) -> Option<(&Element, Option<&str>)> {
        let column = node.attrs.get("drawing.element")?.column.as_ref()?;
        let Some(Column::Drawings { element, shown }) = self.columns.get(column) else { return None };
        let on = shown.iter().find(|(_, frames)| frames.iter().any(|frames| frames.contains(&self.first_frame)));
        Some((self.element(*element)?, on.map(|(name, _)| name.as_str())))
    }
}

/// The palette files a scene's list of palettes names, under `folder`. Its first line says what
/// the file is; each line after names a palette by its path first, in quotes when it has spaces.
fn palette_list(list: &str, folder: &Path) -> Vec<PathBuf> {
    list.lines()
        .skip(1)
        .filter_map(|line| {
            let line = line.trim();
            let path = match line.strip_prefix('"') {
                Some(quoted) => quoted.split('"').next()?,
                None => line.split_whitespace().next()?,
            };
            Some(folder.join(format!("{path}.plt")))
        })
        .collect()
}

/// The value of keys on a frame: the last key's at or before it, held, or eased in a straight
/// line to the next; before the first key, the first key's.
fn value_at(keys: &[Key], frame: u32) -> Option<f64> {
    let after = keys.iter().position(|key| key.frame > frame);
    match after {
        Some(0) => keys.first().map(|key| key.value),
        Some(next) => {
            let (a, b) = (keys[next - 1], keys[next]);
            if a.held || a.frame == frame {
                return Some(a.value);
            }
            let t = f64::from(frame - a.frame) / f64::from(b.frame - a.frame);
            Some(a.value + (b.value - a.value) * t)
        }
        None => keys.last().map(|key| key.value),
    }
}

/// The first child of `node` named `name`.
fn child<'a, 'input>(node: roxmltree::Node<'a, 'input>, name: &str) -> Option<roxmltree::Node<'a, 'input>> {
    node.children().find(|child| child.has_tag_name(name))
}

/// The points of a column, wherever under it they are listed.
fn points<'a, 'input>(column: roxmltree::Node<'a, 'input>) -> Vec<roxmltree::Node<'a, 'input>> {
    column.descendants().filter(|child| child.has_tag_name("points")).flat_map(|points| points.children().filter(|child| child.has_tag_name("pt"))).collect()
}

/// A list of frames such as `1-3,10,55-56`.
fn frame_list(list: &str) -> Vec<RangeInclusive<u32>> {
    list.split(',')
        .filter_map(|part| {
            let part = part.trim();
            match part.split_once('-') {
                Some((from, to)) => Some(from.trim().parse().ok()?..=to.trim().parse().ok()?),
                None => part.parse().ok().map(|frame| frame..=frame),
            }
        })
        .collect()
}

fn read_column(column: roxmltree::Node) -> Column {
    match column.attribute("type") {
        // Drawings: each run of frames names the drawing it shows.
        Some("0") => {
            let sequences: Vec<_> = column.children().filter(|child| child.has_tag_name("elementSeq")).collect();
            let element = sequences.iter().find_map(|sequence| sequence.attribute("id")?.parse().ok());
            let shown =
                sequences.iter().filter_map(|sequence| Some((sequence.attribute("val")?.to_string(), frame_list(sequence.attribute("exposures")?)))).collect();
            element.map_or(Column::Other, |element| Column::Drawings { element, shown })
        }
        // Keys: a point names the frames it is a key on, and its value there.
        Some("3") => {
            let mut keys = Vec::new();
            for point in points(column) {
                let (Some(frames), Some(value)) = (point.attribute("x"), point.attribute("y").and_then(|y| y.parse().ok())) else { continue };
                let held = point.attribute("constSeg") == Some("true");
                keys.extend(frame_list(frames).into_iter().flatten().map(|frame| Key { frame, value, held }));
            }
            keys.sort_by_key(|key| key.frame);
            Column::Keys(keys)
        }
        // A path: points locked to frames.
        Some("2") => {
            let mut path: Vec<(u32, [f64; 3])> = points(column)
                .into_iter()
                .filter_map(|point| {
                    let frame = point.attribute("lockedInTime")?.parse().ok()?;
                    let at: Vec<f64> = point.attribute("val")?.split(',').filter_map(|part| part.trim().parse().ok()).collect();
                    Some((frame, [*at.first()?, *at.get(1)?, at.get(2).copied().unwrap_or(0.0)]))
                })
                .collect();
            path.sort_by_key(|(frame, _)| *frame);
            Column::Path(path)
        }
        _ => Column::Other,
    }
}

fn read_group(group: roxmltree::Node) -> Group {
    let (mut nodes, mut links, mut backdrops) = (Vec::new(), Vec::new(), Vec::new());
    for child in group.children().filter(roxmltree::Node::is_element) {
        match child.tag_name().name() {
            "nodeslist" => {
                for node in child.children().filter(roxmltree::Node::is_element) {
                    let name = node.attribute("name").unwrap_or_default().to_string();
                    let pos = node.attribute("pos").and_then(point);
                    match node.tag_name().name() {
                        "module" => {
                            let kind = node.attribute("type").unwrap_or_default().to_string();
                            let attrs = node.children().find(|child| child.has_tag_name("attrs")).map(read_attr).unwrap_or_default();
                            nodes.push(RigNode { kind, name, attrs, group: None, pos });
                        }
                        "group" => nodes.push(RigNode { kind: GROUP.to_string(), name, attrs: Attr::default(), group: Some(read_group(node)), pos }),
                        _ => {}
                    }
                }
            }
            "linkedlist" => {
                for link in child.children().filter(|child| child.has_tag_name("link")) {
                    let (Some(from), Some(to)) = (link.attribute("out"), link.attribute("in")) else { continue };
                    let port = link.attribute("inport").and_then(|port| port.parse().ok());
                    let out_port = link.attribute("outport").and_then(|port| port.parse().ok());
                    links.push(Link { from: from.to_string(), to: to.to_string(), port, out_port });
                }
            }
            "backdrops" => {
                for backdrop in child.children().filter(|child| child.has_tag_name("backdrop")) {
                    let (Some([x0, y0]), Some([x1, y1])) = (backdrop.attribute("topLeft").and_then(point), backdrop.attribute("bottomRight").and_then(point))
                    else {
                        continue;
                    };
                    let title = backdrop.children().find(|child| child.has_tag_name("title")).and_then(|title| title.attribute("text")).unwrap_or_default();
                    backdrops.push(Backdrop { title: title.to_string(), rect: [x0, y0, x1, y1] });
                }
            }
            _ => {}
        }
    }
    Group { name: group.attribute("name").unwrap_or_default().to_string(), nodes, links, backdrops }
}

/// A place in the node view, as the file writes it: `x,y` or `x,y,z`, the depth left out.
fn point(text: &str) -> Option<[f64; 2]> {
    let mut numbers = text.split(',').map(|number| number.trim().parse::<f64>().ok());
    Some([numbers.next()??, numbers.next()??])
}

fn read_attr(node: roxmltree::Node) -> Attr {
    Attr {
        value: node.attribute("val").map(str::to_string),
        column: node.attribute("col").map(str::to_string),
        children: node.children().filter(roxmltree::Node::is_element).map(|child| (child.tag_name().name().to_string(), read_attr(child))).collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A small scene written for the tests, laid out as such files are: a peg over a drawing,
    /// both in a group, wired into a composite at the top.
    const SCENE: &str = r##"<?xml version="1.0" encoding="UTF-8"?>
<project version="2204">
 <elements>
  <element id="2" elementName="ARM" elementFolder="ARM" rootFolder="elements">
   <drawings><dwg name="1"/><dwg name="2"/><dwg name="open"/></drawings>
  </element>
 </elements>
 <options>
  <metrics unitAspectRatioX="4" unitAspectRatioY="3" numberOfUnitsX="24" numberOfUnitsY="24" numberOfUnitsZ="12"/>
  <resolution name="HD" size="1920,1080"/>
  <framerate val="25"/>
 </options>
 <scenes>
  <scene name="Top" nbframes="60" startFrame="1" stopFrame="60">
   <columns>
    <column type="0" name="COL-DRAWING">
     <elementSeq exposures="2-5" val="1" id="2"/>
     <elementSeq exposures="1,6-9" val="open" id="2"/>
    </column>
    <column type="3" name="COL-X">
     <points version="2">
      <pt constSeg="true" x="1-3,10" yLocal="2.5" y="2.5"/>
      <pt constSeg="false" x="20" yLocal="4" y="4"/>
     </points>
    </column>
    <column type="3" name="COL-ANGLE">
     <points version="2">
      <pt constSeg="false" x="5" y="10"/>
      <pt constSeg="false" x="15" y="30"/>
     </points>
    </column>
    <column type="2" name="COL-PATH">
     <path3D><points><pt val="7,8,0.5" lockedInTime="1"/><pt val="9,9,0" lockedInTime="12"/></points></path3D>
    </column>
    <column type="10" name="COL-SCRIPT"><value><![CDATA[scripts/a.js]]></value></column>
   </columns>
   <rootgroup name="Top">
    <nodeslist>
     <module type="COMPOSITE" name="Composite"><attrs><compositeMode val="compositePassthrough"/></attrs></module>
     <group name="CHARACTER">
      <nodeslist>
       <module type="MULTIPORT_IN" name="Multi-Port-In"/>
       <module type="PEG" name="ARM-P">
        <attrs>
         <position><separate val="true"/><x val="9" col="COL-X"/><y val="-1.5"/><z val="0.25"/></position>
         <scale><x val="-1"/><y val="1"/></scale>
         <angle val="0" col="COL-ANGLE"/>
         <pivot><x val="-1.68"/><y val="0.5"/></pivot>
        </attrs>
       </module>
       <module type="PEG" name="ON-PATH">
        <attrs><position><separate val="false"/><attr3dpath col="COL-PATH"/></position></attrs>
       </module>
       <module type="READ" name="ARM">
        <attrs><drawing><elementMode val="true"/><element col="COL-DRAWING"><layer/></element></drawing><readLineArt val="true"/></attrs>
       </module>
       <module type="MULTIPORT_OUT" name="Multi-Port-Out"/>
      </nodeslist>
      <linkedlist>
       <link out="ARM-P" in="ARM"/>
       <link out="ARM" in="Multi-Port-Out"/>
      </linkedlist>
     </group>
    </nodeslist>
    <linkedlist>
     <link out="CHARACTER" in="Composite" inport="0"/>
    </linkedlist>
   </rootgroup>
  </scene>
 </scenes>
</project>
"##;

    fn scene() -> RigScene {
        RigScene::parse(SCENE, Path::new("/scenes/hero")).expect("the scene is read")
    }

    fn node<'a>(scene: &'a RigScene, name: &str) -> &'a RigNode {
        scene.nodes().into_iter().find(|(_, node)| node.name == name).expect("the node is there").1
    }

    #[test]
    fn a_scene_is_read_with_its_camera_grid_and_elements() {
        let scene = scene();
        assert_eq!((scene.camera, scene.frame_rate, scene.first_frame), ([1920, 1080], 25.0, 1));
        assert_eq!((scene.units, scene.unit_aspect), ([24.0, 24.0, 12.0], [4.0, 3.0]));
        let arm = scene.element(2).unwrap();
        assert_eq!((arm.name.as_str(), arm.drawings.clone()), ("ARM", vec!["1".to_string(), "2".to_string(), "open".to_string()]));
        assert_eq!(arm.drawing_file(&scene, "open"), Path::new("/scenes/hero/elements/ARM/ARM-open.tvg"));
    }

    #[test]
    fn nodes_are_read_in_their_groups_with_their_links() {
        let scene = scene();
        let all: Vec<(String, &str, &str)> = scene.nodes().iter().map(|(path, node)| (path.join("/"), node.kind.as_str(), node.name.as_str())).collect();
        assert_eq!(
            all,
            [
                ("".to_string(), "COMPOSITE", "Composite"),
                ("".to_string(), GROUP, "CHARACTER"),
                ("CHARACTER".to_string(), "MULTIPORT_IN", "Multi-Port-In"),
                ("CHARACTER".to_string(), "PEG", "ARM-P"),
                ("CHARACTER".to_string(), "PEG", "ON-PATH"),
                ("CHARACTER".to_string(), "READ", "ARM"),
                ("CHARACTER".to_string(), "MULTIPORT_OUT", "Multi-Port-Out"),
            ]
        );
        assert_eq!(scene.top.links, [Link { from: "CHARACTER".into(), to: "Composite".into(), port: Some(0), out_port: None }]);
        let inner = scene.top.nodes[1].group.as_ref().unwrap();
        assert_eq!(inner.links[0], Link { from: "ARM-P".into(), to: "ARM".into(), port: None, out_port: None });
        assert_eq!(node(&scene, "Composite").attrs.get("compositeMode").and_then(|mode| mode.value.as_deref()), Some("compositePassthrough"));
    }

    #[test]
    fn groups_are_looked_through_to_what_is_wired_into_each_node() {
        // A peg outside a group carries a drawing inside it, through the group's in port; a
        // composite takes the drawing through the group's out port, and a second input of its
        // own whose port is not written.
        let text = r#"<project><options><resolution size="640,480"/></options><scenes><scene startFrame="1"><rootgroup name="Top">
            <nodeslist>
             <module type="PEG" name="ROOT-P"/>
             <module type="COMPOSITE" name="Composite"/>
             <module type="READ" name="BG"/>
             <group name="CHARACTER"><nodeslist>
              <module type="MULTIPORT_IN" name="Multi-Port-In"/>
              <module type="PEG" name="ARM-P"/>
              <group name="Deformation-ARM"><nodeslist>
               <module type="MULTIPORT_IN" name="Multi-Port-In"/>
               <module type="OffsetModule" name="Offset"/>
               <module type="MULTIPORT_OUT" name="Multi-Port-Out"/>
              </nodeslist><linkedlist>
               <link out="Multi-Port-In" in="Offset"/>
               <link out="Offset" in="Multi-Port-Out"/>
              </linkedlist></group>
              <module type="READ" name="ARM"/>
              <module type="MULTIPORT_OUT" name="Multi-Port-Out"/>
             </nodeslist><linkedlist>
              <link out="Multi-Port-In" in="ARM-P"/>
              <link out="ARM-P" in="Deformation-ARM"/>
              <link out="Deformation-ARM" in="ARM"/>
              <link out="ARM" in="Multi-Port-Out"/>
             </linkedlist></group>
            </nodeslist>
            <linkedlist>
             <link out="ROOT-P" in="CHARACTER"/>
             <link out="CHARACTER" in="Composite" inport="0"/>
             <link out="BG" in="Composite"/>
            </linkedlist>
           </rootgroup></scene></scenes></project>"#;
        let scene = RigScene::parse(text, Path::new("")).unwrap();
        let wired = scene.wired();
        let named = |index: usize| wired[index].node.name.as_str();
        let inputs = |name: &str| -> Vec<(u32, &str)> {
            let node = wired.iter().find(|wired| wired.node.name == name).expect("wired");
            node.inputs.iter().map(|(port, from)| (*port, named(*from))).collect()
        };
        let names: Vec<&str> = wired.iter().map(|wired| wired.node.name.as_str()).collect();
        assert_eq!(names, ["ROOT-P", "Composite", "BG", "ARM-P", "Offset", "ARM"], "no groups and no ports");
        assert_eq!(inputs("ARM-P"), [(0, "ROOT-P")], "in through the group's in port");
        assert_eq!(inputs("Offset"), [(0, "ARM-P")], "into a group within it");
        assert_eq!(inputs("ARM"), [(0, "Offset")], "out of the inner group");
        assert_eq!(inputs("Composite"), [(0, "ARM"), (1, "BG")], "out through the group's out port; the next port where none is written");
        assert_eq!(wired.iter().find(|wired| wired.node.name == "Offset").unwrap().path, ["CHARACTER", "Deformation-ARM"]);
    }

    #[test]
    fn values_are_read_as_they_are_on_the_first_frame() {
        let scene = scene();
        let peg = node(&scene, "ARM-P");
        // Animated: the key on frame 1, not the value written beside the column's name.
        assert_eq!(scene.position(peg), [2.5, -1.5, 0.25]);
        // Before its first key, a value is that key's.
        assert_eq!(scene.number(peg, "angle"), Some(10.0));
        assert_eq!((peg.attrs.number("scale.x"), peg.attrs.number("pivot.x")), (Some(-1.0), Some(-1.68)));
        assert_eq!(scene.number(peg, "skew"), None, "not in the file");
        // A peg on a path is where the path is on the first frame.
        assert_eq!(scene.position(node(&scene, "ON-PATH")), [7.0, 8.0, 0.5]);
        // A drawing node shows the drawing whose frames hold the first.
        let (element, shown) = scene.drawing(node(&scene, "ARM")).unwrap();
        assert_eq!((element.name.as_str(), shown), ("ARM", Some("open")));
    }

    #[test]
    fn keys_and_frame_lists_are_read_as_written() {
        assert_eq!(frame_list("1-3,10, 55-56,x"), [1..=3, 10..=10, 55..=56]);
        let keys = [Key { frame: 1, value: 0.0, held: false }, Key { frame: 11, value: 10.0, held: true }, Key { frame: 21, value: 50.0, held: false }];
        assert_eq!([0, 1, 6, 11, 15, 30].map(|frame| value_at(&keys, frame)), [0.0, 0.0, 5.0, 10.0, 10.0, 50.0].map(Some));
        assert_eq!(value_at(&[], 1), None);
    }

    #[test]
    fn a_scenes_palettes_are_found_by_its_list_or_in_its_folder() {
        let list = "ToonBoomAnimationInc PaletteList 1\n\
                    palette-library/HERO LINK \"E:/elsewhere/palette-library/HERO.plt\"\n\
                    \"palette-library/HERO EYES\" LINK \"E:/elsewhere/palette-library/HERO EYES.plt\"\n\n";
        let folder = Path::new("/scenes/hero");
        assert_eq!(palette_list(list, folder), [folder.join("palette-library/HERO.plt"), folder.join("palette-library/HERO EYES.plt")]);
        // With no list, those in the folder.
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("palette-library")).unwrap();
        for name in ["B.plt", "A.plt", "notes.txt"] {
            std::fs::write(dir.path().join("palette-library").join(name), "").unwrap();
        }
        let mut scene = scene();
        scene.folder = dir.path().to_path_buf();
        assert_eq!(scene.palettes(), [dir.path().join("palette-library/A.plt"), dir.path().join("palette-library/B.plt")]);
    }

    #[test]
    fn a_file_that_is_no_scene_is_turned_down() {
        assert!(RigScene::parse("not xml", Path::new("")).unwrap_err().contains("not a file this import reads"));
        assert!(RigScene::parse("<project/>", Path::new("")).unwrap_err().contains("no camera size"));
        let no_scene = r#"<project><options><resolution size="640,480"/></options></project>"#;
        assert!(RigScene::parse(no_scene, Path::new("")).unwrap_err().contains("no scene in it"));
    }

    /// Reads a real scene, given by its path in `ISKRA_RIG_SCENE`, and says what is in it:
    /// `ISKRA_RIG_SCENE=/path/to/scene.xstage cargo test -p iskra-io a_real_scene -- --ignored --nocapture`.
    #[test]
    #[ignore = "needs a scene of the user's, by its path in ISKRA_RIG_SCENE"]
    fn a_real_scene_is_read_whole() {
        let path = std::env::var("ISKRA_RIG_SCENE").expect("ISKRA_RIG_SCENE names a scene file");
        let scene = RigScene::read(Path::new(&path)).unwrap();
        let nodes = scene.nodes();
        let mut kinds: Vec<(&str, usize)> = Vec::new();
        for (_, node) in &nodes {
            match kinds.iter_mut().find(|(kind, _)| *kind == node.kind) {
                Some((_, count)) => *count += 1,
                None => kinds.push((&node.kind, 1)),
            }
        }
        kinds.sort_by_key(|(kind, count)| (std::cmp::Reverse(*count), *kind));
        println!("camera {:?} at {} fps, grid {:?} of {:?}, {} elements", scene.camera, scene.frame_rate, scene.units, scene.unit_aspect, scene.elements.len());
        println!("{} nodes: {kinds:?}", nodes.len());
        let mut missing = Vec::new();
        for (path, node) in &nodes {
            if node.kind == "READ" {
                match scene.drawing(node) {
                    Some((element, shown)) => {
                        let file = shown.map(|shown| element.drawing_file(&scene, shown));
                        if file.as_ref().is_some_and(|file| !file.exists()) {
                            missing.push(format!("{}/{}", path.join("/"), node.name));
                        }
                    }
                    None => missing.push(format!("{}/{} (no element)", path.join("/"), node.name)),
                }
            }
            if node.kind == "PEG" {
                let [x, y, z] = scene.position(node);
                assert!(x.is_finite() && y.is_finite() && z.is_finite(), "{}", node.name);
            }
        }
        println!("drawing nodes whose drawing on the first frame has no file: {missing:?}");
        let wired = scene.wired();
        let fed = wired.iter().filter(|wired| !wired.inputs.is_empty()).count();
        let unfed: Vec<String> = wired
            .iter()
            .filter(|wired| wired.inputs.is_empty() && matches!(wired.node.kind.as_str(), "READ" | "COMPOSITE" | "CUTTER"))
            .map(|wired| wired.node.name.clone())
            .collect();
        println!(
            "{} nodes with groups looked through, {fed} with something wired in; drawing nodes, composites and cutters with nothing: {unfed:?}",
            wired.len()
        );
        for path in scene.palettes() {
            let read = std::fs::read_to_string(&path).map_err(|why| why.to_string()).and_then(|text| crate::palette_of_plt(&text));
            match read {
                Ok(plt) => {
                    println!("palette {}: {} colours, {} textures, left out {:?}", path.display(), plt.palette.swatches.len(), plt.textures.len(), plt.left)
                }
                Err(why) => println!("palette {}: not read, {why}", path.display()),
            }
        }
        assert!(!nodes.is_empty() && !scene.elements.is_empty());
    }
}
