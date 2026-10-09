//! The utility network graph: nodes and links on `cp_core` primitives (arena ids, cubic
//! curves), without lanes (`system-design.md` §10).
//!
//! Water and sewage share **one pipe network** (as in CS1); power lines are a second
//! network. Both live in one [`UtilityGraph`] so [`UtilityLinkId`]s are unique (the
//! `RemoveLink` command needs no network argument); every node and link carries its
//! [`UtilityNet`], and links never join nodes of different networks, so connected
//! components are per network by construction.
//!
//! **Components**: [`UtilityGraph::components`] labels every node of a network by BFS in
//! node-arena order; ids are assigned in the order of each component's lowest node, so
//! they are deterministic. Nothing is stored: the capacity-distribution task (T-038) can
//! call it (or cache it itself) after any change.

use bevy_ecs::resource::Resource;
use bevy_math::Vec2;
use cp_content::UtilityKind;
use cp_core::curve::{CubicBezier3, to_xz};
use cp_core::{Arena, Id};
use serde::{Deserialize, Serialize};

pub type UtilityNodeId = Id<UtilityNode>;
pub type UtilityLinkId = Id<UtilityLink>;

/// Which network a node or link belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum UtilityNet {
    /// Water and sewage.
    Pipe,
    Power,
}

impl UtilityNet {
    pub fn of(kind: UtilityKind) -> Self {
        match kind {
            UtilityKind::Water | UtilityKind::Sewage => Self::Pipe,
            UtilityKind::Power => Self::Power,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct UtilityNode {
    pub net: UtilityNet,
    /// World position (Y = ground height at build time).
    pub pos: bevy_math::Vec3,
    /// Attached links in attachment order.
    pub links: Vec<UtilityLinkId>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct UtilityLink {
    pub net: UtilityNet,
    pub a: UtilityNodeId,
    pub b: UtilityNodeId,
    pub curve: CubicBezier3,
    /// Arc length, metres.
    pub length: f32,
}

/// Node labels of one network's connected components.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Components {
    /// `(node, component)` in node-arena order.
    pub labels: Vec<(UtilityNodeId, u32)>,
    pub count: u32,
}

impl Components {
    pub fn of(&self, node: UtilityNodeId) -> Option<u32> {
        self.labels
            .iter()
            .find(|(n, _)| *n == node)
            .map(|(_, c)| *c)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GraphError {
    UnknownLink(UtilityLinkId),
    UnknownNode(UtilityNodeId),
    /// A split position outside `(0, 1)` or not increasing.
    BadSplit,
}

impl std::fmt::Display for GraphError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownLink(l) => write!(f, "unknown utility link {l:?}"),
            Self::UnknownNode(n) => write!(f, "unknown utility node {n:?}"),
            Self::BadSplit => write!(f, "invalid split position"),
        }
    }
}

impl std::error::Error for GraphError {}

#[derive(Resource, Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct UtilityGraph {
    nodes: Arena<UtilityNode>,
    links: Arena<UtilityLink>,
}

impl UtilityGraph {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn node(&self, id: UtilityNodeId) -> Option<&UtilityNode> {
        self.nodes.get(id)
    }
    pub fn link(&self, id: UtilityLinkId) -> Option<&UtilityLink> {
        self.links.get(id)
    }
    /// Nodes in arena (index) order.
    pub fn nodes(&self) -> impl Iterator<Item = (UtilityNodeId, &UtilityNode)> {
        self.nodes.iter()
    }
    pub fn links(&self) -> impl Iterator<Item = (UtilityLinkId, &UtilityLink)> {
        self.links.iter()
    }
    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }
    pub fn link_count(&self) -> usize {
        self.links.len()
    }

    /// Nearest node of `net` within `radius` (XZ), ties by id.
    pub fn nearest_node(&self, net: UtilityNet, p: Vec2, radius: f32) -> Option<UtilityNodeId> {
        let mut best: Option<(f32, UtilityNodeId)> = None;
        for (id, n) in self.nodes.iter() {
            if n.net != net {
                continue;
            }
            let d = to_xz(n.pos).distance(p);
            if d <= radius && best.is_none_or(|(bd, _)| d < bd) {
                best = Some((d, id));
            }
        }
        best.map(|(_, id)| id)
    }

    /// Nearest point on a link of `net` within `radius` (XZ): `(link, t, distance)`.
    pub fn nearest_link(
        &self,
        net: UtilityNet,
        p: Vec2,
        radius: f32,
    ) -> Option<(UtilityLinkId, f32, f32)> {
        let mut best: Option<(UtilityLinkId, f32, f32)> = None;
        for (id, l) in self.links.iter() {
            if l.net != net {
                continue;
            }
            let (t, d) = l.curve.nearest_xz(p);
            if d <= radius && best.is_none_or(|(_, _, bd)| d < bd) {
                best = Some((id, t, d));
            }
        }
        best
    }

    pub fn add_node(&mut self, net: UtilityNet, pos: bevy_math::Vec3) -> UtilityNodeId {
        self.nodes.insert(UtilityNode {
            net,
            pos,
            links: Vec::new(),
        })
    }

    /// Links `a` and `b` (same network) along `curve`. Returns the existing link if the two
    /// nodes are already directly linked.
    pub fn add_link(
        &mut self,
        a: UtilityNodeId,
        b: UtilityNodeId,
        curve: CubicBezier3,
    ) -> Result<UtilityLinkId, GraphError> {
        let net = self.nodes.get(a).ok_or(GraphError::UnknownNode(a))?.net;
        self.nodes.get(b).ok_or(GraphError::UnknownNode(b))?;
        if let Some(existing) = self
            .links
            .iter()
            .find_map(|(id, l)| ((l.a == a && l.b == b) || (l.a == b && l.b == a)).then_some(id))
        {
            return Ok(existing);
        }
        let length = curve.arc_table().length();
        let id = self.links.insert(UtilityLink {
            net,
            a,
            b,
            curve,
            length,
        });
        for n in [a, b] {
            self.nodes.get_mut(n).expect("checked").links.push(id);
        }
        Ok(id)
    }

    /// Removes a link; nodes left without links are removed with it.
    pub fn remove_link(&mut self, id: UtilityLinkId) -> Result<(), GraphError> {
        let link = self.links.remove(id).ok_or(GraphError::UnknownLink(id))?;
        for n in [link.a, link.b] {
            if let Some(node) = self.nodes.get_mut(n) {
                node.links.retain(|&l| l != id);
                if node.links.is_empty() {
                    self.nodes.remove(n);
                }
            }
        }
        Ok(())
    }

    /// Splits a link at the strictly increasing parameters `ts` in `(0, 1)`; the link is
    /// replaced by `ts.len() + 1` links joined by new nodes (returned in order).
    pub fn split_link(
        &mut self,
        id: UtilityLinkId,
        ts: &[f32],
    ) -> Result<Vec<UtilityNodeId>, GraphError> {
        let link = self
            .links
            .get(id)
            .ok_or(GraphError::UnknownLink(id))?
            .clone();
        let mut prev = 0.0;
        for &t in ts {
            if !(t > prev && t < 1.0) {
                return Err(GraphError::BadSplit);
            }
            prev = t;
        }
        let mut bounds = vec![0.0];
        bounds.extend_from_slice(ts);
        bounds.push(1.0);
        // Remove the old link without pruning its end nodes.
        self.links.remove(id);
        for n in [link.a, link.b] {
            if let Some(node) = self.nodes.get_mut(n) {
                node.links.retain(|&l| l != id);
            }
        }
        let mut chain = vec![link.a];
        let mut new_nodes = Vec::new();
        for &t in ts {
            let n = self.add_node(link.net, link.curve.point(t));
            chain.push(n);
            new_nodes.push(n);
        }
        chain.push(link.b);
        for (i, w) in bounds.windows(2).enumerate() {
            let piece = link.curve.sub_curve(w[0], w[1]);
            let length = piece.arc_table().length();
            let lid = self.links.insert(UtilityLink {
                net: link.net,
                a: chain[i],
                b: chain[i + 1],
                curve: piece,
                length,
            });
            for n in [chain[i], chain[i + 1]] {
                self.nodes.get_mut(n).expect("exists").links.push(lid);
            }
        }
        Ok(new_nodes)
    }

    /// Connected components of one network.
    pub fn components(&self, net: UtilityNet) -> Components {
        let mut labels: Vec<(UtilityNodeId, u32)> = Vec::new();
        let mut label_of: std::collections::BTreeMap<UtilityNodeId, u32> = Default::default();
        let mut count = 0;
        for (start, n) in self.nodes.iter() {
            if n.net != net || label_of.contains_key(&start) {
                continue;
            }
            let mut queue = std::collections::VecDeque::from([start]);
            label_of.insert(start, count);
            while let Some(cur) = queue.pop_front() {
                for &lid in &self.nodes.get(cur).expect("exists").links {
                    let l = self.links.get(lid).expect("node links exist");
                    let other = if l.a == cur { l.b } else { l.a };
                    if let std::collections::btree_map::Entry::Vacant(e) = label_of.entry(other) {
                        e.insert(count);
                        queue.push_back(other);
                    }
                }
            }
            count += 1;
        }
        for (id, n) in self.nodes.iter() {
            if n.net == net {
                labels.push((id, label_of[&id]));
            }
        }
        Components { labels, count }
    }

    /// Whether two nodes are in the same component.
    pub fn connected(&self, a: UtilityNodeId, b: UtilityNodeId) -> bool {
        let Some(na) = self.nodes.get(a) else {
            return false;
        };
        let c = self.components(na.net);
        match (c.of(a), c.of(b)) {
            (Some(x), Some(y)) => x == y,
            _ => false,
        }
    }
}
