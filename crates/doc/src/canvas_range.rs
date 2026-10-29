//! A read document brought within the canvas: what lies beyond [`MAX_COORD`] is removed (#3575).
//! Edits refuse to leave anything there (`doc_sane` in the engine checks the artboards, the guides,
//! the ruler origin and the objects an edit touches), so a file holding something beyond it could be
//! opened but not edited: one object, artboard or guide out there made every move fail.

use std::sync::Arc;

use vectorcraft_geom::{Point, Rect};

use crate::{Document, MAX_COORD, Node, NodeKind};

/// What [`Document::drop_beyond_canvas`] removed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BeyondCanvas {
    /// Objects removed whole (a layer or group keeps its children within the canvas).
    pub objects: usize,
    pub artboards: usize,
    /// Guides beyond the canvas, and the artboard guides of the artboards removed.
    pub guides: usize,
    /// The ruler origin was beyond the canvas and is back at the origin.
    pub ruler_origin: bool,
}

impl BeyondCanvas {
    /// The warning for what was removed, or `None` when nothing was.
    pub fn note(&self) -> Option<String> {
        let count = |n: usize, one: &str, many: &str| match n {
            0 => None,
            1 => Some(format!("1 {one}")),
            n => Some(format!("{n} {many}")),
        };
        let parts: Vec<String> =
            [count(self.objects, "object", "objects"), count(self.artboards, "artboard", "artboards"), count(self.guides, "guide", "guides")]
                .into_iter()
                .flatten()
                .collect();
        let limit = format!("more than {MAX_COORD} pt from the origin");
        match (parts.is_empty(), self.ruler_origin) {
            (true, false) => None,
            (true, true) => Some(format!("the ruler origin lay beyond the canvas ({limit}) and was reset")),
            (false, origin) => Some(format!(
                "{} beyond the canvas ({limit}) {} removed{}",
                parts.join(", "),
                if self.objects + self.artboards + self.guides == 1 { "was" } else { "were" },
                if origin { "; the ruler origin, beyond it too, was reset" } else { "" }
            )),
        }
    }
}

/// Is every side of `r` finite and within [`MAX_COORD`] of the origin?
fn rect_in_range(r: Rect) -> bool {
    [r.x0, r.y0, r.x1, r.y1].iter().all(|v| in_range(*v))
}

fn in_range(v: f64) -> bool {
    v.is_finite() && v.abs() <= MAX_COORD
}

/// Does `n` reach beyond the canvas? An object without geometry doesn't.
fn beyond(n: &Node) -> bool {
    n.geometric_bounds().is_some_and(|b| !rect_in_range(b))
}

/// Remove the children of `node` that reach beyond the canvas: a layer or group that isn't
/// clipping keeps those of its own children within it, anything else goes whole. Only a node
/// with such a child is copied. Recursion follows the nesting, which [`Document::limit_depth`]
/// bounds.
fn drop_children_beyond(node: &mut Arc<Node>, dropped: &mut usize) {
    if !node.children().is_some_and(|c| c.iter().any(|c| beyond(c))) {
        return;
    }
    let Some(children) = Arc::make_mut(node).children_mut() else { return };
    children.retain_mut(|c| {
        if !beyond(c) {
            return true;
        }
        if matches!(c.kind, NodeKind::Layer { clip: false, .. } | NodeKind::Group { clip: false, .. }) {
            drop_children_beyond(c, dropped);
            return true;
        }
        *dropped += 1;
        false
    });
}

impl Document {
    /// Remove what a read file has beyond the canvas ([`MAX_COORD`], or not finite): objects
    /// (see [`BeyondCanvas::objects`]), artboards, and guides with the artboard guides of those
    /// artboards, and put a ruler origin beyond it back at the origin. Run after
    /// [`Document::limit_depth`]. A document whose artboards were all removed has none left.
    pub fn drop_beyond_canvas(&mut self) -> BeyondCanvas {
        let mut gone = BeyondCanvas::default();
        for layer in &mut self.layers {
            drop_children_beyond(layer, &mut gone.objects);
        }
        let removed: Vec<u32> = self.artboards.iter().filter(|a| !rect_in_range(a.rect)).map(|a| a.id).collect();
        self.artboards.retain(|a| rect_in_range(a.rect));
        gone.artboards = removed.len();
        let guides = self.guides.len();
        self.guides.retain(|g| in_range(g.pos) && g.artboard.is_none_or(|a| !removed.contains(&a)));
        gone.guides = guides - self.guides.len();
        if !(in_range(self.ruler_origin.x) && in_range(self.ruler_origin.y)) {
            self.ruler_origin = Point::ZERO;
            gone.ruler_origin = true;
        }
        gone
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Appearance, Artboard, Guide, NodeId};
    use vectorcraft_geom::shapes;

    fn rect(id: u64, y: f64) -> Arc<Node> {
        Arc::new(Node::path(NodeId(id), shapes::rectangle(Rect::new(0.0, y, 10.0, y + 10.0)), Appearance::default_art()))
    }

    fn doc(children: Vec<Arc<Node>>) -> Document {
        let mut d = Document::new(100.0, 100.0);
        if let Some(ch) = d.layers.first_mut().and_then(|l| Arc::make_mut(l).children_mut()) {
            ch.extend(children);
        }
        d
    }

    fn ids(d: &Document) -> Vec<u64> {
        let mut out = vec![];
        fn walk(n: &Node, out: &mut Vec<u64>) {
            out.push(n.id.0);
            for c in n.children().into_iter().flatten() {
                walk(c, out);
            }
        }
        for l in &d.layers {
            walk(l, &mut out);
        }
        out
    }

    #[test]
    fn a_document_within_the_canvas_is_left_alone() {
        let mut d = doc(vec![rect(10, 0.0), rect(11, MAX_COORD - 10.0), rect(12, -MAX_COORD)]);
        let before = d.clone();
        assert_eq!(d.drop_beyond_canvas(), BeyondCanvas::default());
        assert_eq!(d, before);
        assert_eq!(BeyondCanvas::default().note(), None);
    }

    #[test]
    fn objects_beyond_the_canvas_are_removed_and_groups_keep_the_rest() {
        let group = Arc::new(Node::group(NodeId(20), vec![rect(21, 0.0), rect(22, 1e17)]));
        let clip = Arc::new(Node::new(NodeId(30), NodeKind::Group { children: vec![rect(31, f64::NAN), rect(32, 0.0)], clip: true }));
        let mut d = doc(vec![rect(10, 0.0), rect(11, 1e17), group, clip, rect(12, -MAX_COORD - 1.0)]);
        let layer = d.layers[0].id.0;
        let gone = d.drop_beyond_canvas();
        assert_eq!(gone, BeyondCanvas { objects: 4, ..Default::default() });
        assert_eq!(ids(&d), [layer, 10, 20, 21], "the group keeps its object within the canvas; the clip group goes with its clipping path");
        assert!(d.layers.iter().all(|l| !beyond(l)));
        assert_eq!(gone.note().unwrap(), format!("4 objects beyond the canvas (more than {MAX_COORD} pt from the origin) were removed"));
    }

    #[test]
    fn artboards_guides_and_the_ruler_origin_beyond_the_canvas_are_removed_or_reset() {
        let mut d = doc(vec![rect(10, 0.0)]);
        let far = Artboard { id: 9, rect: Rect::new(0.0, 1e17, 100.0, 1e17 + 100.0), ..d.artboards[0].clone() };
        d.artboards.push(far);
        let guide = |pos: f64, artboard| Guide { artboard, ..Guide::new(false, pos) };
        d.guides = vec![guide(50.0, None), guide(1e17, None), guide(f64::INFINITY, None), guide(20.0, Some(9))];
        d.ruler_origin = Point::new(1e17, 0.0);
        let gone = d.drop_beyond_canvas();
        assert_eq!(gone, BeyondCanvas { objects: 0, artboards: 1, guides: 3, ruler_origin: true });
        assert_eq!(d.artboards.len(), 1);
        assert_eq!(d.guides, [guide(50.0, None)], "the far artboard's own guide goes with it");
        assert_eq!(d.ruler_origin, Point::ZERO);
        assert_eq!(
            gone.note().unwrap(),
            format!(
                "1 artboard, 3 guides beyond the canvas (more than {MAX_COORD} pt from the origin) were removed; the ruler origin, beyond it too, was reset"
            )
        );
    }
}
