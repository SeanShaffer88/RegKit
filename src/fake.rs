//! In-memory fake backend — the host-test substitute for the
//! real registry. It models keys as a map from full path to
//! that key's values, derives subkeys from path prefixes,
//! and honours the `Backend` contracts (including
//! `delete_key` refusing a key that still has subkeys, the
//! way RegDeleteKey does).

use std::cell::RefCell;
use std::collections::BTreeMap;

use crate::{Backend, KeyListing, RegValue, RgResult, Root, ERR_NOT_FOUND, ERR_PLATFORM};

#[derive(Default)]
pub struct FakeBackend {
    /// (root, full key path) -> (value name -> value).
    /// Value name "" is the key's default value.
    keys: RefCell<BTreeMap<(Root, String), BTreeMap<String, RegValue>>>,
}

impl FakeBackend {
    pub fn new() -> Self {
        FakeBackend::default()
    }

    /// Test helper: does this exact key exist?
    pub fn has_key(&self, root: Root, path: &str) -> bool {
        self.keys.borrow().contains_key(&(root, path.to_string()))
    }

    /// Test helper: number of keys under a root.
    pub fn key_count(&self) -> usize {
        self.keys.borrow().len()
    }
}

impl Backend for FakeBackend {
    fn key_exists(&self, root: Root, path: &str) -> RgResult<bool> {
        Ok(self.keys.borrow().contains_key(&(root, path.to_string())))
    }

    fn read_value(&self, root: Root, path: &str, name: Option<&str>) -> RgResult<Option<RegValue>> {
        let keys = self.keys.borrow();
        let values = keys.get(&(root, path.to_string())).ok_or((
            ERR_NOT_FOUND,
            format!("key not found: {}\\{path}", root.as_str()),
        ))?;
        Ok(values.get(name.unwrap_or("")).cloned())
    }

    fn list(&self, root: Root, path: &str) -> RgResult<KeyListing> {
        let keys = self.keys.borrow();
        let values = keys.get(&(root, path.to_string())).ok_or((
            ERR_NOT_FOUND,
            format!("key not found: {}\\{path}", root.as_str()),
        ))?;
        let prefix = format!("{path}\\");
        let mut subkeys: Vec<String> = Vec::new();
        for (r, p) in keys.keys() {
            if *r == root {
                if let Some(rest) = p.strip_prefix(&prefix) {
                    if !rest.contains('\\') {
                        subkeys.push(rest.to_string());
                    }
                }
            }
        }
        subkeys.sort();
        Ok(KeyListing {
            subkeys,
            values: values.iter().map(|(n, v)| (n.clone(), v.clone())).collect(),
        })
    }

    fn create_key(&self, root: Root, path: &str) -> RgResult<()> {
        let mut keys = self.keys.borrow_mut();
        let mut cur = String::new();
        for seg in path.split('\\') {
            if !cur.is_empty() {
                cur.push('\\');
            }
            cur.push_str(seg);
            keys.entry((root, cur.clone())).or_default();
        }
        Ok(())
    }

    fn set_value(
        &self,
        root: Root,
        path: &str,
        name: Option<&str>,
        value: &RegValue,
    ) -> RgResult<()> {
        self.create_key(root, path)?;
        let mut keys = self.keys.borrow_mut();
        keys.entry((root, path.to_string()))
            .or_default()
            .insert(name.unwrap_or("").to_string(), value.clone());
        Ok(())
    }

    fn delete_value(&self, root: Root, path: &str, name: Option<&str>) -> RgResult<()> {
        let mut keys = self.keys.borrow_mut();
        let values = keys.get_mut(&(root, path.to_string())).ok_or((
            ERR_NOT_FOUND,
            format!("key not found: {}\\{path}", root.as_str()),
        ))?;
        if values.remove(name.unwrap_or("")).is_none() {
            return Err((
                ERR_NOT_FOUND,
                format!("value not set at {}\\{path}", root.as_str()),
            ));
        }
        Ok(())
    }

    fn delete_key(&self, root: Root, path: &str) -> RgResult<()> {
        let mut keys = self.keys.borrow_mut();
        if !keys.contains_key(&(root, path.to_string())) {
            return Err((
                ERR_NOT_FOUND,
                format!("key not found: {}\\{path}", root.as_str()),
            ));
        }
        let prefix = format!("{path}\\");
        if keys
            .keys()
            .any(|(r, p)| *r == root && p.starts_with(&prefix))
        {
            return Err((
                ERR_PLATFORM,
                format!("key {}\\{path} still has subkeys", root.as_str()),
            ));
        }
        keys.remove(&(root, path.to_string()));
        Ok(())
    }
}
