use std::collections::HashMap;
use std::sync::{Arc, Mutex};

pub struct Inner { items: Vec<u8> }
impl Inner {
    pub fn new() -> Self { Self { items: Vec::new() } }
    pub fn is_empty(&self) -> bool { self.items.is_empty() }
}

pub struct Driver { inner: Arc<Mutex<Inner>>, map: HashMap<u8, u8>, other: Inner }
impl Driver {
    pub fn new() -> Self {
        let inner = Arc::new(Mutex::new(Inner::new()));
        let map = HashMap::new();
        let v: Vec<u8> = Vec::new();
        let _ = String::new();
        Self { inner, map, other: Inner::new() }
    }
    pub fn is_empty(&self) -> bool { self.map.is_empty() && self.other.is_empty() }
}

pub fn walk(n: u32) -> u32 { if n == 0 { 0 } else { walk(n - 1) + walk(n - 2) } }

pub enum Node { Leaf(u32), List(Vec<Node>), Wrap(Box<Node>) }

pub fn total(n: &Node) -> u32 {
    match n {
        Node::Leaf(v) => *v,
        Node::List(items) => {
            let mut t = 0;
            for i in items {
                t += total(i);
            }
            t
        }
        Node::Wrap(inner) => total(inner),
    }
}

pub mod views {
    pub fn draw() {
        super::panels::draw();
    }
}

pub mod panels {
    pub fn draw() {}
}

pub fn part(part: &str) -> bool {
    is_final(part) && is_final(part)
}

fn is_final(p: &str) -> bool {
    p.is_empty()
}
