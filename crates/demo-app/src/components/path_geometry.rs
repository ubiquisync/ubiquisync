//! Geometry for local editing, using the same handle interpretation as the renderer.
use super::sub_path::ResolvedVertex;
use crate::state::XY;

#[derive(Clone, Copy, Debug)]
pub(super) struct Point(pub f64, pub f64);

impl Point {
    fn mix(self, other: Self, t: f64) -> Self {
        Self(
            self.0 + (other.0 - self.0) * t,
            self.1 + (other.1 - self.1) * t,
        )
    }
    fn distance_squared(self, other: Self) -> f64 {
        (self.0 - other.0).powi(2) + (self.1 - other.1).powi(2)
    }
    fn rounded(self) -> XY {
        XY {
            x: self.0.round() as i64,
            y: self.1.round() as i64,
        }
    }
    fn offset(self, origin: XY) -> XY {
        Point(self.0 - origin.x as f64, self.1 - origin.y as f64).rounded()
    }
}

impl From<XY> for Point {
    fn from(p: XY) -> Self {
        Self(p.x as f64, p.y as f64)
    }
}

fn absolute(p: XY, offset: XY) -> Point {
    Point(p.x as f64 + offset.x as f64, p.y as f64 + offset.y as f64)
}

pub(super) fn controls(a: ResolvedVertex, b: ResolvedVertex) -> [Point; 4] {
    let start = Point::from(a.pos);
    let end = Point::from(b.pos);
    let (c1, c2) = match (a.out_handle, b.in_handle) {
        (Some(a_out), Some(b_in)) => (absolute(a.pos, a_out), absolute(b.pos, b_in)),
        (Some(offset), None) => {
            let q = absolute(a.pos, offset);
            (start.mix(q, 2.0 / 3.0), end.mix(q, 2.0 / 3.0))
        }
        (None, Some(offset)) => {
            let q = absolute(b.pos, offset);
            (start.mix(q, 2.0 / 3.0), end.mix(q, 2.0 / 3.0))
        }
        (None, None) => (start.mix(end, 1.0 / 3.0), start.mix(end, 2.0 / 3.0)),
    };
    [start, c1, c2, end]
}

pub(super) fn evaluate(c: [Point; 4], t: f64) -> Point {
    let a = c[0].mix(c[1], t);
    let b = c[1].mix(c[2], t);
    let d = c[2].mix(c[3], t);
    a.mix(b, t).mix(b.mix(d, t), t)
}

/// Approximate the closest parameter, refining every sampled local minimum so
/// loops and sharply bent curves are not restricted to one initial guess.
pub(super) fn nearest_parameter(c: [Point; 4], target: Point) -> f64 {
    const SAMPLES: usize = 96;
    let distance = |t| evaluate(c, t).distance_squared(target);
    let samples: Vec<_> = (0..=SAMPLES)
        .map(|i| distance(i as f64 / SAMPLES as f64))
        .collect();
    let mut best = if samples[0] <= samples[SAMPLES] {
        0.0
    } else {
        1.0
    };
    for i in 1..SAMPLES {
        if samples[i] > samples[i - 1] || samples[i] > samples[i + 1] {
            continue;
        }
        let mut lo = (i - 1) as f64 / SAMPLES as f64;
        let mut hi = (i + 1) as f64 / SAMPLES as f64;
        for _ in 0..32 {
            let a = lo + (hi - lo) / 3.0;
            let b = hi - (hi - lo) / 3.0;
            if distance(a) < distance(b) {
                hi = b;
            } else {
                lo = a;
            }
        }
        let t = (lo + hi) / 2.0;
        if distance(t) < distance(best) {
            best = t;
        }
    }
    best
}

pub(super) struct Split {
    pub vertex: ResolvedVertex,
    pub outgoing: Option<XY>,
    pub incoming: Option<XY>,
}

/// de Casteljau subdivision. Quadratics are elevated to cubics first;
/// each resulting segment therefore has unambiguous endpoint handles.
pub(super) fn split(a: ResolvedVertex, b: ResolvedVertex, t: f64) -> Split {
    let c = controls(a, b);
    let p01 = c[0].mix(c[1], t);
    let p12 = c[1].mix(c[2], t);
    let p23 = c[2].mix(c[3], t);
    let p012 = p01.mix(p12, t);
    let p123 = p12.mix(p23, t);
    let pos = p012.mix(p123, t).rounded();
    let curved = a.out_handle.is_some() || b.in_handle.is_some();
    Split {
        vertex: ResolvedVertex {
            pos,
            in_handle: curved.then(|| p012.offset(pos)),
            out_handle: curved.then(|| p123.offset(pos)),
        },
        outgoing: curved.then(|| p01.offset(a.pos)),
        incoming: curved.then(|| p23.offset(b.pos)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_preserve_lines_and_both_quadratic_forms_and_cubics() {
        for (out_handle, in_handle) in [
            (None, None),
            (Some(XY { x: 100, y: -180 }), None),
            (None, Some(XY { x: -130, y: 220 })),
            (Some(XY { x: 100, y: -180 }), Some(XY { x: -130, y: 220 })),
        ] {
            let a = ResolvedVertex {
                pos: XY { x: 20, y: 40 },
                in_handle: None,
                out_handle,
            };
            let b = ResolvedVertex {
                pos: XY { x: 350, y: 110 },
                in_handle,
                out_handle: None,
            };
            for t in [0.1, 0.37, 0.5, 0.91] {
                let s = split(a, b, t);
                let left = controls(
                    ResolvedVertex {
                        out_handle: s.outgoing,
                        ..a
                    },
                    s.vertex,
                );
                let right = controls(
                    s.vertex,
                    ResolvedVertex {
                        in_handle: s.incoming,
                        ..b
                    },
                );
                for i in 0..=100 {
                    let u = i as f64 / 100.0;
                    let actual = if u <= t {
                        evaluate(left, u / t)
                    } else {
                        evaluate(right, (u - t) / (1.0 - t))
                    };
                    // XY is integer-valued: subdivision rounds coordinates.
                    assert!(actual.distance_squared(evaluate(controls(a, b), u)) < 2.0);
                }
            }
        }
    }

    #[test]
    fn nearest_parameter_projects_to_line_and_curve() {
        let line = [
            Point(0.0, 0.0),
            Point(100.0, 0.0),
            Point(200.0, 0.0),
            Point(300.0, 0.0),
        ];
        assert!((nearest_parameter(line, Point(75.0, 8.0)) - 0.25).abs() < 1e-6);
        assert_eq!(nearest_parameter(line, Point(-20.0, 0.0)), 0.0);
        let curve = [
            Point(0.0, 0.0),
            Point(10.0, 200.0),
            Point(210.0, -100.0),
            Point(300.0, 0.0),
        ];
        assert!((nearest_parameter(curve, evaluate(curve, 0.63)) - 0.63).abs() < 1e-6);
    }
}
