//! Slider path math: a direct port of osu-framework's `PathApproximator` /
//! `CircularArcProperties` and osu!lazer's `SliderPath`.
//! All vector math is f32, matching osuTK `Vector2`.

use crate::vec2::{precision, Vec2};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SplineType {
    Catmull,
    Bezier,
    Linear,
    PerfectCurve,
    BSpline,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PathType {
    pub spline: SplineType,
    pub degree: Option<i32>,
}

impl PathType {
    pub const CATMULL: PathType = PathType { spline: SplineType::Catmull, degree: None };
    pub const BEZIER: PathType = PathType { spline: SplineType::Bezier, degree: None };
    pub const LINEAR: PathType = PathType { spline: SplineType::Linear, degree: None };
    pub const PERFECT_CURVE: PathType = PathType { spline: SplineType::PerfectCurve, degree: None };
    pub const fn bspline(degree: i32) -> PathType {
        PathType { spline: SplineType::BSpline, degree: Some(degree) }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct PathControlPoint {
    pub position: Vec2,
    pub kind: Option<PathType>,
}

const BEZIER_TOLERANCE: f32 = 0.25;
const CATMULL_DETAIL: usize = 50;
const CIRCULAR_ARC_TOLERANCE: f32 = 0.1;

pub struct PathApproximator;

impl PathApproximator {
    pub fn linear_to_piecewise_linear(points: &[Vec2]) -> Vec<Vec2> {
        points.to_vec()
    }

    /// Port of framework `BSplineToPiecewiseLinear` (which also serves bezier via
    /// `degree = points.len()`; the internal clamp makes it a single bezier).
    pub fn bspline_to_piecewise_linear(points: &[Vec2], degree: i32) -> Vec<Vec2> {
        if points.len() < 2 {
            return points.first().map(|p| vec![*p]).unwrap_or_default();
        }

        let mut degree = degree.min((points.len() - 1) as i32);
        let degree_usize = degree as usize;
        let point_count = points.len() - 1;

        // Segments in stack order: last element popped first = first curve segment.
        let mut to_flatten = Self::b_spline_to_bezier_internal(points, &mut degree);
        let mut output: Vec<Vec2> = Vec::new();

        while let Some(parent) = to_flatten.pop() {
            if Self::bezier_is_flat_enough(&parent) {
                Self::bezier_approximate(&parent, &mut output, degree_usize + 1);
                continue;
            }

            let mut right_child = vec![Vec2::ZERO; degree_usize + 1];
            let mut subdivision_buffer1 = vec![Vec2::ZERO; degree_usize + 1];
            let left_child = Self::bezier_subdivide(&parent, &mut right_child, &mut subdivision_buffer1, degree_usize + 1);

            let new_parent = left_child;
            to_flatten.push(right_child);
            to_flatten.push(new_parent);
        }

        output.push(points[point_count]);
        output
    }

    fn b_spline_to_bezier_internal(points: &[Vec2], degree: &mut i32) -> Vec<Vec<Vec2>> {
        // Returns segments in stack-pop order: last element is popped first,
        // therefore holds the FIRST curve segment.
        *degree = (*degree).min((points.len() - 1) as i32);
        let degree_usize = *degree as usize;
        let point_count = points.len() - 1;
        let mut pts: Vec<Vec2> = points.to_vec();

        if degree_usize == point_count {
            // B-spline subdivision unnecessary, degenerates to a single bezier.
            return vec![pts];
        }

        let mut result: Vec<Vec<Vec2>> = Vec::new();

        for i in 0..point_count - degree_usize {
            let mut sub_bezier = vec![Vec2::ZERO; degree_usize + 1];
            sub_bezier[0] = pts[i];

            // Destructively insert the knot degree-1 times via Boehm's algorithm.
            for j in 0..degree_usize - 1 {
                sub_bezier[j + 1] = pts[i + 1];

                for k in 1..degree_usize - j {
                    let l = k.min(point_count - degree_usize - i);
                    pts[i + k] = (pts[i + k] * l as f32 + pts[i + k + 1]) / (l + 1) as f32;
                }
            }

            sub_bezier[degree_usize] = pts[i + 1];
            result.push(sub_bezier);
        }

        result.push(pts[point_count - degree_usize..].to_vec());
        // Reverse the stack so elements can be accessed in order.
        result.reverse();
        result
    }

    fn bezier_is_flat_enough(points: &[Vec2]) -> bool {
        for i in 1..points.len() - 1 {
            if (points[i - 1] - points[i] * 2.0 + points[i + 1]).length_squared()
                > BEZIER_TOLERANCE * BEZIER_TOLERANCE * 4.0
            {
                return false;
            }
        }
        true
    }

    /// Splits bezier control points into two halves; returns the left half.
    fn bezier_subdivide(
        points: &[Vec2],
        right: &mut [Vec2],
        subdivision_buffer: &mut [Vec2],
        count: usize,
    ) -> Vec<Vec2> {
        let mut midpoints = subdivision_buffer[..count].to_vec();
        let mut left = vec![Vec2::ZERO; count];

        for i in 0..count {
            midpoints[i] = points[i];
        }

        for i in 0..count {
            left[i] = midpoints[0];
            right[count - i - 1] = midpoints[count - i - 1];

            for j in 0..count - i - 1 {
                midpoints[j] = (midpoints[j] + midpoints[j + 1]) / 2.0;
            }
        }

        left
    }

    fn bezier_approximate(points: &[Vec2], output: &mut Vec<Vec2>, count: usize) {
        let mut right = vec![Vec2::ZERO; count];
        let mut subdivision_buffer1 = vec![Vec2::ZERO; count];
        let l = Self::bezier_subdivide(points, &mut right, &mut subdivision_buffer1, count);

        // l[count + i] = r[i + 1] in the original combined buffer.
        let mut combined = l;
        for i in 0..count - 1 {
            combined.push(right[i + 1]);
        }

        output.push(points[0]);

        for i in 1..count - 1 {
            let index = 2 * i;
            let p = (combined[index - 1] + combined[index] * 2.0 + combined[index + 1]) * 0.25;
            output.push(p);
        }
    }

    pub fn catmull_to_piecewise_linear(points: &[Vec2]) -> Vec<Vec2> {
        let mut result = Vec::with_capacity((points.len().saturating_sub(1)) * CATMULL_DETAIL * 2);

        for i in 0..points.len() - 1 {
            let v1 = if i > 0 { points[i - 1] } else { points[i] };
            let v2 = points[i];
            let v3 = if i < points.len() - 1 { points[i + 1] } else { v2 + v2 - v1 };
            let v4 = if i < points.len() - 2 { points[i + 2] } else { v3 + v3 - v2 };

            for c in 0..CATMULL_DETAIL {
                result.push(Self::catmull_find_point(v1, v2, v3, v4, c as f32 / CATMULL_DETAIL as f32));
                result.push(Self::catmull_find_point(v1, v2, v3, v4, (c + 1) as f32 / CATMULL_DETAIL as f32));
            }
        }

        result
    }

    fn catmull_find_point(vec1: Vec2, vec2: Vec2, vec3: Vec2, vec4: Vec2, t: f32) -> Vec2 {
        let t2 = t * t;
        let t3 = t * t2;

        let x = 0.5
            * (2.0 * vec2.x
                + (-vec1.x + vec3.x) * t
                + (2.0 * vec1.x - 5.0 * vec2.x + 4.0 * vec3.x - vec4.x) * t2
                + (-vec1.x + 3.0 * vec2.x - 3.0 * vec3.x + vec4.x) * t3);
        let y = 0.5
            * (2.0 * vec2.y
                + (-vec1.y + vec3.y) * t
                + (2.0 * vec1.y - 5.0 * vec2.y + 4.0 * vec3.y - vec4.y) * t2
                + (-vec1.y + 3.0 * vec2.y - 3.0 * vec3.y + vec4.y) * t3);

        Vec2::new(x, y)
    }

    pub fn circular_arc_to_piecewise_linear(points: &[Vec2]) -> Vec<Vec2> {
        let pr = match CircularArcProperties::new(points) {
            Some(p) => p,
            None => return Self::bspline_to_piecewise_linear(points, points.len() as i32),
        };

        let amount_points = if 2.0 * pr.radius as f64 <= CIRCULAR_ARC_TOLERANCE as f64 {
            2
        } else {
            ((pr.theta_range
                / (2.0 * (1.0 - (CIRCULAR_ARC_TOLERANCE as f64 / pr.radius as f64)).acos()))
            .ceil() as usize)
            .max(2)
        };

        let mut output = Vec::with_capacity(amount_points);
        for i in 0..amount_points {
            let fract = i as f64 / (amount_points - 1) as f64;
            let theta = pr.theta_start + pr.direction * fract * pr.theta_range;
            let o = Vec2::new(theta.cos() as f32, theta.sin() as f32) * pr.radius;
            output.push(pr.centre + o);
        }
        output
    }

    /// Framework `BezierToPiecewiseLinear`.
    pub fn bezier_to_piecewise_linear(points: &[Vec2]) -> Vec<Vec2> {
        Self::bspline_to_piecewise_linear(points, (points.len() as i32 - 1).max(1))
    }
}

struct CircularArcProperties {
    theta_start: f64,
    theta_range: f64,
    direction: f64,
    radius: f32,
    centre: Vec2,
}

impl CircularArcProperties {
    fn new(points: &[Vec2]) -> Option<Self> {
        let a = points[0];
        let b = points[1];
        let c = points[2];

        // Degenerate triangle where a side-length is almost zero: fallback.
        if precision::almost_equals_f64(
            0.0,
            ((b.y - a.y) * (c.x - a.x) - (b.x - a.x) * (c.y - a.y)) as f64,
        ) {
            return None;
        }

        let d = 2.0 * (a.x * (b - c).y + b.x * (c - a).y + c.x * (a - b).y);
        let a_sq = a.length_squared();
        let b_sq = b.length_squared();
        let c_sq = c.length_squared();

        let centre = Vec2::new(
            a_sq * (b - c).y + b_sq * (c - a).y + c_sq * (a - b).y,
            a_sq * (c - b).x + b_sq * (a - c).x + c_sq * (b - a).x,
        ) / d;

        let d_a = a - centre;
        let d_c = c - centre;

        let radius = d_a.length();

        let theta_start = d_a.y.atan2(d_a.x) as f64;
        let mut theta_end = d_c.y.atan2(d_c.x) as f64;

        while theta_end < theta_start {
            theta_end += 2.0 * std::f64::consts::PI;
        }

        let mut direction = 1.0f64;
        let mut theta_range = theta_end - theta_start;

        // Decide in which direction to draw the circle, depending on which side of AC B lies.
        let ortho_a_to_c = c - a;
        let ortho_a_to_c = Vec2::new(ortho_a_to_c.y, -ortho_a_to_c.x);

        if Vec2::dot(ortho_a_to_c, b - a) < 0.0 {
            direction = -direction;
            theta_range = 2.0 * std::f64::consts::PI - theta_range;
        }

        Some(CircularArcProperties { theta_start, theta_range, direction, radius, centre })
    }
}

/// Port of osu!lazer's `SliderPath`. Uses interior mutability for lazily
/// computed state so position queries can take `&self`.
pub struct SliderPath {
    pub control_points: Vec<PathControlPoint>,
    pub expected_distance: Option<f64>,
    pub optimise_catmull: bool,

    computed: std::cell::RefCell<ComputedState>,
}

struct ComputedState {
    calculated_path: Vec<Vec2>,
    cumulative_length: Vec<f64>,
    calculated_length: f64,
    optimised_length: f64,
    valid: bool,
}

impl SliderPath {
    pub fn new(control_points: Vec<PathControlPoint>, expected_distance: Option<f64>) -> Self {
        SliderPath {
            control_points,
            expected_distance,
            optimise_catmull: true,
            computed: std::cell::RefCell::new(ComputedState {
                calculated_path: Vec::new(),
                cumulative_length: Vec::new(),
                calculated_length: 0.0,
                optimised_length: 0.0,
                valid: false,
            }),
        }
    }

    fn ensure_valid(&self) {
        let mut computed = self.computed.borrow_mut();
        if computed.valid {
            return;
        }
        self.calculate_path(&mut computed);
        self.calculate_length(&mut computed);
        computed.valid = true;
    }

    /// The distance of the path after lengthening/shortening for `expected_distance`.
    pub fn distance(&self) -> f64 {
        self.ensure_valid();
        let computed = self.computed.borrow();
        if computed.cumulative_length.is_empty() {
            0.0
        } else {
            *computed.cumulative_length.last().unwrap()
        }
    }

    /// The full piecewise-linear calculated path (`SliderPath.CalculatedPath`).
    pub fn calculated_path(&self) -> Vec<Vec2> {
        self.ensure_valid();
        self.computed.borrow().calculated_path.clone()
    }

    /// Port of `SliderPath.GetPathToProgress`: the piecewise-linear sub-path
    /// between progress `p0` and `p1` (both clamped to 0..1). Used for
    /// rendering snaking slider bodies.
    pub fn path_to_progress(&self, p0: f64, p1: f64) -> Vec<Vec2> {
        self.ensure_valid();
        let computed = self.computed.borrow();

        let total = if computed.cumulative_length.is_empty() {
            0.0
        } else {
            *computed.cumulative_length.last().unwrap()
        };
        let d0 = p0.clamp(0.0, 1.0) * total;
        let d1 = p1.clamp(0.0, 1.0) * total;

        let mut path: Vec<Vec2> = Vec::new();

        let mut i = 0usize;
        while i < computed.calculated_path.len() && computed.cumulative_length[i] < d0 {
            i += 1;
        }
        path.push(interpolate_vertices(&computed, i, d0));

        while i < computed.calculated_path.len() && computed.cumulative_length[i] <= d1 {
            path.push(computed.calculated_path[i]);
            i += 1;
        }

        path.push(interpolate_vertices(&computed, i, d1));
        path
    }

    /// Position on the slider path at progress `p` (clamped to 0..1).
    pub fn position_at(&self, progress: f64) -> Vec2 {
        self.ensure_valid();

        let computed = self.computed.borrow();
        let d = progress.clamp(0.0, 1.0)
            * if computed.cumulative_length.is_empty() {
                0.0
            } else {
                *computed.cumulative_length.last().unwrap()
            };
        let i = index_of_distance(&computed.cumulative_length, d);
        interpolate_vertices(&computed, i, d)
    }

    fn calculate_path(&self, computed: &mut ComputedState) {
        computed.calculated_path.clear();
        computed.optimised_length = 0.0;

        if self.control_points.is_empty() {
            return;
        }

        let vertices: Vec<Vec2> = self.control_points.iter().map(|c| c.position).collect();

        let mut start = 0usize;
        for i in 0..self.control_points.len() {
            if self.control_points[i].kind.is_none() && i < self.control_points.len() - 1 {
                continue;
            }

            // The current vertex ends the segment.
            let segment_vertices = &vertices[start..=i];
            let segment_type = self.control_points[start].kind.unwrap_or(PathType::LINEAR);

            if segment_vertices.len() == 1 {
                computed.calculated_path.push(segment_vertices[0]);
            } else {
                let (sub_path, optimised_add) =
                    self.calculate_sub_path(segment_vertices, segment_type);
                computed.optimised_length += optimised_add;

                let skip_first = !computed.calculated_path.is_empty()
                    && !sub_path.is_empty()
                    && *computed.calculated_path.last().unwrap() == sub_path[0];

                let begin = if skip_first { 1 } else { 0 };
                for p in &sub_path[begin..] {
                    computed.calculated_path.push(*p);
                }
            }

            start = i;
        }
    }

    /// Returns (points, extra optimised length for catmull optimisation).
    fn calculate_sub_path(&self, sub: &[Vec2], path_type: PathType) -> (Vec<Vec2>, f64) {
        match path_type.spline {
            SplineType::Linear => (PathApproximator::linear_to_piecewise_linear(sub), 0.0),
            SplineType::PerfectCurve => {
                if sub.len() == 3 {
                    if let Some(props) = CircularArcProperties::new(sub) {
                        let sub_points = if 2.0 * props.radius as f64 <= CIRCULAR_ARC_TOLERANCE as f64 {
                            2usize
                        } else {
                            ((props.theta_range
                                / (2.0 * (1.0 - (CIRCULAR_ARC_TOLERANCE as f64 / props.radius as f64)).acos()))
                            .ceil() as usize)
                                .max(2)
                        };

                        if sub_points < 1000 {
                            let sub_path = PathApproximator::circular_arc_to_piecewise_linear(sub);
                            if !sub_path.is_empty() {
                                return (sub_path, 0.0);
                            }
                        }
                    }
                }

                // Fall back to bezier (framework `BezierToPiecewiseLinear` semantics).
                let pts = PathApproximator::bspline_to_piecewise_linear(sub, sub.len() as i32);
                (pts, 0.0)
            }
            SplineType::Catmull => {
                let sub_path = PathApproximator::catmull_to_piecewise_linear(sub);

                if !self.optimise_catmull {
                    return (sub_path, 0.0);
                }

                // Basic form of stable's 6px catmull optimisation.
                let mut optimised_path = Vec::with_capacity(sub_path.len());
                let mut last_start: Option<Vec2> = None;
                let mut length_removed_since_start = 0.0f64;
                let mut optimised_length = 0.0f64;

                const CATMULL_SEGMENT_LENGTH: usize = CATMULL_DETAIL * 2;

                for i in 0..sub_path.len() {
                    let point = sub_path[i];
                    match last_start {
                        None => {
                            optimised_path.push(point);
                            last_start = Some(point);
                        }
                        Some(start_point) => {
                            let dist_from_start = Vec2::distance(start_point, point) as f64;
                            length_removed_since_start +=
                                Vec2::distance(sub_path[i - 1], point) as f64;

                            if dist_from_start > 6.0
                                || (i + 1) % CATMULL_SEGMENT_LENGTH == 0
                                || i == sub_path.len() - 1
                            {
                                optimised_path.push(point);
                                optimised_length += length_removed_since_start - dist_from_start;

                                last_start = None;
                                length_removed_since_start = 0.0;
                            }
                        }
                    }
                }

                (optimised_path, optimised_length)
            }
            _ => {
                // Bezier / B-spline.
                let pts = PathApproximator::bspline_to_piecewise_linear(
                    sub,
                    path_type.degree.unwrap_or(sub.len() as i32),
                );
                (pts, 0.0)
            }
        }
    }

    fn calculate_length(&self, computed: &mut ComputedState) {
        computed.calculated_length = computed.optimised_length;
        computed.cumulative_length.clear();
        computed.cumulative_length.push(0.0);

        for i in 0..computed.calculated_path.len() - 1 {
            let diff = computed.calculated_path[i + 1] - computed.calculated_path[i];
            computed.calculated_length += diff.length() as f64;
            computed.cumulative_length.push(computed.calculated_length);
        }

        if let Some(expected_distance) = self.expected_distance {
            if computed.calculated_length != expected_distance {
                // In osu-stable, if the last two path points of a slider are equal,
                // extension is not performed.
                if computed.calculated_path.len() >= 2
                    && *computed.calculated_path.last().unwrap()
                        == computed.calculated_path[computed.calculated_path.len() - 2]
                    && expected_distance > computed.calculated_length
                {
                    computed.cumulative_length.push(computed.calculated_length);
                    return;
                }

                // The last length is always incorrect.
                computed.cumulative_length.pop();

                let mut path_end_index = computed.calculated_path.len() - 1;

                if computed.calculated_length > expected_distance {
                    while !computed.cumulative_length.is_empty()
                        && *computed.cumulative_length.last().unwrap() >= expected_distance
                    {
                        computed.cumulative_length.pop();
                        computed.calculated_path.remove(path_end_index);
                        if path_end_index == 0 {
                            break;
                        }
                        path_end_index -= 1;
                    }
                }

                if path_end_index == 0 {
                    computed.cumulative_length.push(0.0);
                    return;
                }

                // The direction of the segment to shorten or lengthen.
                let dir = (computed.calculated_path[path_end_index]
                    - computed.calculated_path[path_end_index - 1])
                    .normalized();

                computed.calculated_path[path_end_index] = computed.calculated_path[path_end_index - 1]
                    + dir * (expected_distance - *computed.cumulative_length.last().unwrap()) as f32;
                computed.cumulative_length.push(expected_distance);
            }
        }
    }
}

fn index_of_distance(cumulative_length: &[f64], d: f64) -> usize {
    // total_cmp keeps a deterministic order even with NaN lengths
    // (degenerate curves can produce them).
    match cumulative_length.binary_search_by(|v| v.total_cmp(&d)) {
        Ok(i) => i,
        Err(i) => i,
    }
}

fn interpolate_vertices(computed: &ComputedState, i: usize, d: f64) -> Vec2 {
    if computed.calculated_path.is_empty() {
        return Vec2::ZERO;
    }

    if i == 0 {
        return computed.calculated_path[0];
    }
    if i >= computed.calculated_path.len() {
        return *computed.calculated_path.last().unwrap();
    }

    let p0 = computed.calculated_path[i - 1];
    let p1 = computed.calculated_path[i];

    let d0 = computed.cumulative_length[i - 1];
    let d1 = computed.cumulative_length[i];

    if precision::almost_equals_f64(d0, d1) {
        return p0;
    }

    let w = (d - d0) / (d1 - d0);
    p0 + (p1 - p0) * w as f32
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vec2::Vec2;

    #[test]
    fn single_point_curve_with_expected_distance() {
        let cps = vec![PathControlPoint { position: Vec2::ZERO, kind: Some(PathType::LINEAR) }];
        let mut p = SliderPath::new(cps, Some(2000.0));
        let _ = p.distance();
        let _ = p.position_at(0.5);
    }
}
