use std::fs;
use std::io;
use std::path::Path;
use toml_edit::{ArrayOfTables, DocumentMut, Item, Table};

pub fn merge_settings_from_template(settings_path: &Path, template_src: &str) -> io::Result<bool> {
    let user_src = fs::read_to_string(settings_path)?;
    let mut user_doc: DocumentMut = user_src
        .parse()
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    let tpl_doc: DocumentMut = template_src
        .parse()
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    let changed = merge_item(user_doc.as_item_mut(), tpl_doc.as_item());
    if changed {
        fs::write(settings_path, user_doc.to_string())?;
    }
    Ok(changed)
}

fn merge_item(user: &mut Item, tpl: &Item) -> bool {
    match (user, tpl) {
        (Item::Table(user_t), Item::Table(tpl_t)) => merge_table(user_t, tpl_t),
        (user_item, tpl_item) if user_item.is_none() => {
            *user_item = tpl_item.clone();
            true
        }
        _ => false,
    }
}

fn merge_table(user: &mut Table, tpl: &Table) -> bool {
    let mut changed = false;
    for (k, _tpl_item) in tpl.iter() {
        if !user.contains_key(k) {
            let (tpl_key, tpl_item) = tpl.get_key_value(k).expect("iter key exists");
            user.insert_formatted(tpl_key, tpl_item.clone());
            changed = true;
            continue;
        }
        let user_item = user.get_mut(k).unwrap();
        let (_tpl_key, tpl_item) = tpl.get_key_value(k).unwrap();
        match (user_item, tpl_item) {
            (Item::Table(user_t), Item::Table(tpl_t)) => {
                if merge_table(user_t, tpl_t) {
                    changed = true;
                }
            }
            (Item::ArrayOfTables(user_aot), Item::ArrayOfTables(tpl_aot)) => {
                if merge_aot(user_aot, tpl_aot) {
                    changed = true;
                }
            }
            _ => {}
        }
    }
    changed
}

fn merge_aot(user: &mut ArrayOfTables, tpl: &ArrayOfTables) -> bool {
    let mut changed = false;
    for (u, t) in user.iter_mut().zip(tpl.iter()) {
        if merge_table(u, t) {
            changed = true;
        }
    }
    if tpl.len() > user.len() {
        for t in tpl.iter().skip(user.len()) {
            user.push(t.clone());
            changed = true;
        }
    }
    changed
}
