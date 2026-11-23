//! A smooth curve through points, which tone curves and pressure curves both are.

/// What `tone` comes out as through a curve of `points`, which are in order: on the smooth
/// line through them that keeps between each two what they are (a monotone cubic), level
/// before the first and after the last. With no points it is the tone itself.
pub fn monotone_through(points: &[[f64; 2]], tone: f64) -> f64 {
    let (Some(first), Some(last)) = (points.first(), points.last()) else { return tone };
    if tone <= first[0] || points.len() == 1 {
        return first[1];
    }
    if tone >= last[0] {
        return last[1];
    }
    // The steepness of each stretch, and at each point the one the line has there: none at
    // a turn, so that it never goes past a point, and else between those of its two sides.
    let steep = |at: usize| (points[at + 1][1] - points[at][1]) / (points[at + 1][0] - points[at][0]);
    let wide = |at: usize| points[at + 1][0] - points[at][0];
    let slope = |at: usize| {
        if at == 0 {
            return steep(0);
        }
        if at == points.len() - 1 {
            return steep(at - 1);
        }
        let (before, after) = (steep(at - 1), steep(at));
        if before * after <= 0.0 {
            return 0.0;
        }
        let (a, b) = (2.0 * wide(at) + wide(at - 1), wide(at) + 2.0 * wide(at - 1));
        (a + b) / (a / before + b / after)
    };
    let at = points.partition_point(|point| point[0] <= tone) - 1;
    let (width, t) = (wide(at), (tone - points[at][0]) / wide(at));
    let (from, to, out, into) = (points[at][1], points[at + 1][1], slope(at) * width, slope(at + 1) * width);
    let (t2, t3) = (t * t, t * t * t);
    let value = (2.0 * t3 - 3.0 * t2 + 1.0) * from + (t3 - 2.0 * t2 + t) * out + (-2.0 * t3 + 3.0 * t2) * to + (t3 - t2) * into;
    // An end's slope is its stretch's own, which can carry the line a little past a point.
    value.clamp(from.min(to), from.max(to))
}
