//! Bin commands (plan §7.11): the bins the project's items are ordered in. Master, the bin
//! everything starts in, is named by leaving the bin out, or by `null`.

use iskra_model::{Bin, BinId, Item, Project, unique_name};
use serde::Deserialize;
use serde_json::{Value, json};

use super::{checked_name, invalid, parse, title};
use crate::{Result, Session};

/// The bin a command names, which the project has to have. `None` is Master.
pub(super) fn bin_in(project: &Project, bin: Option<u64>) -> Result<Option<BinId>> {
    match bin.map(BinId) {
        Some(id) if project.bin(id).is_none() => invalid(format!("no bin {}", id.0)),
        bin => Ok(bin),
    }
}

/// `name` has to be free among the bins of `parent`, or be the name the bin `own` has.
fn name_is_free(project: &Project, parent: Option<BinId>, name: &str, own: Option<BinId>) -> Result<()> {
    if project.bins_in(parent).iter().any(|bin| Some(bin.id) != own && bin.name == name) {
        return invalid(format!("there is a bin named `{name}` there already"));
    }
    Ok(())
}

/// Makes a bin, in `parent` or else in Master. Without a name it gets a free "Bin" one.
pub(super) fn new(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        name: Option<String>,
        parent: Option<u64>,
    }
    let id = "bin.new";
    let Params { name, parent } = parse(id, params)?;
    let bin = s.edit(title(id), |p| {
        let parent = bin_in(p, parent)?;
        let name = match &name {
            Some(name) => {
                let name = checked_name(name)?;
                name_is_free(p, parent, name, None)?;
                name.to_string()
            }
            None => unique_name("Bin", p.bins_in(parent).iter().map(|bin| bin.name.as_str())),
        };
        let id = BinId(p.new_id());
        p.bins.push(Bin { id, name, parent });
        Ok(id)
    })?;
    Ok(json!({ "id": bin }))
}

pub(super) fn rename(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        bin: u64,
        name: String,
    }
    let id = "bin.rename";
    let Params { bin, name } = parse(id, params)?;
    let name = checked_name(&name)?;
    s.edit(title(id), |p| {
        let Some(bin) = bin_in(p, Some(bin))? else { return Ok(()) };
        name_is_free(p, p.bin(bin).and_then(|bin| bin.parent), name, Some(bin))?;
        if let Some(bin) = p.bins.iter_mut().find(|other| other.id == bin) {
            bin.name = name.to_string();
        }
        Ok(())
    })?;
    Ok(Value::Null)
}

/// Deletes a bin. The items and bins it held are then in the bin it was in; nothing of the
/// project is deleted with it.
pub(super) fn delete(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        bin: u64,
    }
    let id = "bin.delete";
    let Params { bin } = parse(id, params)?;
    s.edit(title(id), |p| match p.remove_bin(BinId(bin)) {
        Some(()) => Ok(()),
        None => invalid(format!("no bin {bin}")),
    })?;
    Ok(Value::Null)
}

/// Puts a bin into another, or into Master. A bin can't go into itself or into one it holds.
pub(super) fn set_parent(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        bin: u64,
        parent: Option<u64>,
    }
    let id = "bin.setParent";
    let Params { bin, parent } = parse(id, params)?;
    s.edit(title(id), |p| {
        let (Some(bin), parent) = (bin_in(p, Some(bin))?, bin_in(p, parent)?) else { return Ok(()) };
        if p.bin_holds(bin, parent) {
            return invalid("a bin can't be put into itself or into a bin that is in it");
        }
        let name = p.bin(bin).map(|bin| bin.name.clone()).unwrap_or_default();
        name_is_free(p, parent, &name, Some(bin))?;
        if let Some(bin) = p.bins.iter_mut().find(|other| other.id == bin) {
            bin.parent = parent;
        }
        Ok(())
    })?;
    Ok(Value::Null)
}

/// Puts items (each `{ "shot": id }` or `{ "storyboard": id }`) into a bin, or into Master.
pub(super) fn set_bin(s: &mut Session, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct Params {
        items: Vec<Item>,
        bin: Option<u64>,
    }
    let id = "item.setBin";
    let Params { items, bin } = parse(id, params)?;
    s.edit(title(id), |p| {
        let bin = bin_in(p, bin)?;
        for item in &items {
            if p.set_bin(*item, bin).is_none() {
                return invalid("the project has no such item");
            }
        }
        Ok(())
    })?;
    Ok(Value::Null)
}
