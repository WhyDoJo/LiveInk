use bytemuck::{Pod, Zeroable};
use egui::{Color32, Pos2, Vec2, pos2};
use std::sync::Arc;

pub const COLORS: [Color32; 6] = [
    Color32::from_rgb(84, 192, 255),
    Color32::from_rgb(255, 92, 153),
    Color32::from_rgb(250, 218, 94),
    Color32::from_rgb(151, 230, 120),
    Color32::from_rgb(186, 154, 255),
    Color32::from_rgb(245, 245, 250),
];

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Tool {
    #[default]
    Pen,
    Line,
    Arrow,
    Rectangle,
    Ellipse,
    Eraser,
}

impl Tool {
    pub const ALL: [Self; 6] = [
        Self::Pen,
        Self::Line,
        Self::Arrow,
        Self::Rectangle,
        Self::Ellipse,
        Self::Eraser,
    ];
    pub fn label(self) -> &'static str {
        match self {
            Self::Pen => "Pen · P",
            Self::Line => "Line · L",
            Self::Arrow => "Arrow · A",
            Self::Rectangle => "Rectangle · R",
            Self::Ellipse => "Ellipse · O",
            Self::Eraser => "Stroke eraser · E",
        }
    }
}

#[derive(Clone)]
pub struct Stroke {
    pub tool: Tool,
    pub points: Vec<Pos2>,
    pub color: Color32,
    pub width: f32,
}

// One analytic SDF primitive. No CPU tessellation or per-segment draw calls.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable, Debug)]
pub struct Instance {
    pub bounds: [f32; 4],
    pub color: [f32; 4],
    pub data: [f32; 4],
    pub params: [f32; 4],
}

impl Instance {
    fn segment(a: Pos2, b: Pos2, width: f32, color: Color32) -> Self {
        let pad = width * 0.5 + 1.5;
        Self {
            bounds: [
                a.x.min(b.x) - pad,
                a.y.min(b.y) - pad,
                (a.x - b.x).abs() + pad * 2.,
                (a.y - b.y).abs() + pad * 2.,
            ],
            color: color.to_normalized_gamma_f32(),
            data: [a.x, a.y, b.x, b.y],
            params: [0., width * 0.5, 0., 0.],
        }
    }
}

impl Stroke {
    pub fn end(&self) -> Pos2 {
        *self.points.last().unwrap_or(&Pos2::ZERO)
    }

    pub fn primitives(&self, out: &mut Vec<Instance>) {
        let Some(&a) = self.points.first() else {
            return;
        };
        let b = self.end();
        let mut line = |a, b| out.push(Instance::segment(a, b, self.width, self.color));
        match self.tool {
            Tool::Pen => {
                if self.points.len() == 1 {
                    line(a, a);
                }
                for pair in self.points.windows(2) {
                    line(pair[0], pair[1]);
                }
            }
            Tool::Line | Tool::Arrow => {
                line(a, b);
                if self.tool == Tool::Arrow && a.distance(b) > 2. {
                    let direction = (a - b).normalized();
                    let side = Vec2::new(-direction.y, direction.x);
                    let size = (self.width * 4.).max(16.).min(a.distance(b) * 0.45);
                    line(b, b + direction * size + side * size * 0.5);
                    line(b, b + direction * size - side * size * 0.5);
                }
            }
            Tool::Rectangle => {
                line(a, pos2(b.x, a.y));
                line(pos2(b.x, a.y), b);
                line(b, pos2(a.x, b.y));
                line(pos2(a.x, b.y), a);
            }
            Tool::Ellipse => {
                let center = a.lerp(b, 0.5);
                let radii = (b - a).abs() * 0.5;
                let pad = self.width * 0.5 + 1.5;
                if radii.x < 1. || radii.y < 1. {
                    line(a, b);
                } else {
                    out.push(Instance {
                        bounds: [
                            center.x - radii.x - pad,
                            center.y - radii.y - pad,
                            radii.x * 2. + pad * 2.,
                            radii.y * 2. + pad * 2.,
                        ],
                        color: self.color.to_normalized_gamma_f32(),
                        data: [center.x, center.y, radii.x, radii.y],
                        params: [1., self.width * 0.5, 0., 0.],
                    });
                }
            }
            Tool::Eraser => {}
        }
    }

    pub fn hit(&self, p: Pos2, radius: f32) -> bool {
        // Hit test the same geometry that is actually rendered (including arrowheads).
        let mut instances = Vec::new();
        self.primitives(&mut instances);
        instances.iter().any(|i| {
            let [ax, ay, bx, by] = i.data;
            let distance = if i.params[0] == 0. {
                segment_distance(p, pos2(ax, ay), pos2(bx, by))
            } else {
                let q = p - pos2(ax, ay);
                let k = ((q.x / bx).powi(2) + (q.y / by).powi(2)).sqrt();
                let k1 = ((q.x / (bx * bx)).powi(2) + (q.y / (by * by)).powi(2)).sqrt();
                if k1 < 0.00001 {
                    bx.min(by)
                } else {
                    (k * (k - 1.) / k1).abs()
                }
            };
            distance <= radius + self.width * 0.5
        })
    }
}

fn segment_distance(p: Pos2, a: Pos2, b: Pos2) -> f32 {
    let ab = b - a;
    let t = ((p - a).dot(ab) / ab.length_sq().max(0.0001)).clamp(0., 1.);
    p.distance(a + ab * t)
}

#[derive(Default)]
pub struct Document {
    pub strokes: Vec<Arc<Stroke>>,
    undo: Vec<Vec<Arc<Stroke>>>,
    redo: Vec<Vec<Arc<Stroke>>>,
}

impl Document {
    pub fn checkpoint(&mut self) {
        // Bounded history, sharing immutable stroke data instead of copying all points.
        // Checkpoints happen once per gesture, never per pointer motion.
        if self.undo.len() >= 64 {
            self.undo.remove(0);
        }
        self.undo.push(self.strokes.clone());
        self.redo.clear();
    }
    pub fn clear(&mut self) {
        if !self.strokes.is_empty() {
            self.checkpoint();
            self.strokes.clear();
        }
    }
    pub fn undo(&mut self) {
        if let Some(previous) = self.undo.pop() {
            self.redo
                .push(std::mem::replace(&mut self.strokes, previous));
        }
    }
    pub fn redo(&mut self) {
        if let Some(next) = self.redo.pop() {
            self.undo.push(std::mem::replace(&mut self.strokes, next));
        }
    }
    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }
    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn stroke(tool: Tool) -> Stroke {
        Stroke {
            tool,
            points: vec![pos2(10., 10.), pos2(110., 110.)],
            color: COLORS[0],
            width: 4.,
        }
    }
    #[test]
    fn undo_redo_clear() {
        let mut doc = Document::default();
        doc.checkpoint();
        doc.strokes.push(Arc::new(stroke(Tool::Pen)));
        doc.undo();
        assert!(doc.strokes.is_empty());
        doc.redo();
        assert_eq!(doc.strokes.len(), 1);
        doc.clear();
        assert!(doc.strokes.is_empty());
        doc.undo();
        assert_eq!(doc.strokes.len(), 1);
    }

    #[test]
    fn new_gesture_discards_redo_branch() {
        let mut doc = Document::default();
        doc.checkpoint();
        doc.strokes.push(Arc::new(stroke(Tool::Line)));
        doc.undo();
        assert!(doc.can_redo());
        doc.checkpoint();
        doc.strokes.push(Arc::new(stroke(Tool::Rectangle)));
        assert!(!doc.can_redo());
    }

    #[test]
    fn history_is_bounded_and_shares_points() {
        let mut doc = Document::default();
        let original = Arc::new(stroke(Tool::Pen));
        doc.strokes.push(original.clone());
        for _ in 0..100 {
            doc.checkpoint();
        }
        assert_eq!(doc.undo.len(), 64);
        assert!(Arc::ptr_eq(&doc.undo[0][0], &original));
    }
    #[test]
    fn shape_hit_testing() {
        assert!(stroke(Tool::Line).hit(pos2(50., 50.), 4.));
        assert!(!stroke(Tool::Rectangle).hit(pos2(60., 60.), 4.));
        assert!(stroke(Tool::Rectangle).hit(pos2(60., 10.), 4.));
        assert!(stroke(Tool::Ellipse).hit(pos2(60., 10.), 4.));
        assert!(!stroke(Tool::Ellipse).hit(pos2(60., 60.), 4.));
    }
    #[test]
    fn degenerate_geometry_stays_finite() {
        for tool in Tool::ALL {
            let mut s = stroke(tool);
            s.points = vec![pos2(20., 20.)];
            let mut instances = Vec::new();
            s.primitives(&mut instances);
            assert!(instances.iter().all(|i| {
                bytemuck::cast_slice::<Instance, f32>(&[*i])
                    .iter()
                    .all(|v| v.is_finite())
            }));
        }
    }
}
