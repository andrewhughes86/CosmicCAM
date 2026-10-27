//! The kernel on simple solids with known volumes.

use super::*;

fn volume(tris: &[[DVec3; 3]]) -> f64 {
    tris.iter().map(|[a, b, c]| a.dot(b.cross(*c)) / 6.0).sum()
}

fn xy() -> Result<Plane> {
    Plane::new(DVec3::ZERO, DVec3::X, DVec3::Y).ok_or_else(|| err("plane"))
}

fn rect(w: f64, h: f64) -> Vec<Segment> {
    let p = [DVec2::ZERO, DVec2::new(w, 0.0), DVec2::new(w, h), DVec2::new(0.0, h)];
    (0..4).map(|i| Segment::Line { a: p[i], b: p[(i + 1) % 4] }).collect()
}

#[test]
fn a_block_with_a_hole_through_it() -> Result<()> {
    let plane = xy()?;
    let block = Body::extrude(&plane, &rect(40.0, 30.0), 0.0, 10.0)?;
    let v = volume(&block.triangles(0.01)?);
    assert!((v - 12_000.0).abs() < 1.0, "{v}");
    let hole = Body::extrude(&plane, &[Segment::Circle { center: DVec2::new(20.0, 15.0), radius: 5.0 }], -1.0, 11.0)?;
    let cut = block.combine(&hole, Operation::Cut, 0.01)?;
    let v = volume(&cut.triangles(0.01)?);
    let exact = 12_000.0 - std::f64::consts::PI * 25.0 * 10.0;
    assert!((v - exact).abs() / exact < 0.005, "{v} vs {exact}");
    Ok(())
}

#[test]
fn profiles_with_holes_arcs_and_a_revolve() -> Result<()> {
    let plane = xy()?;
    // A washer drawn as two circles: the inner one is a hole.
    let washer = Body::extrude(
        &plane,
        &[Segment::Circle { center: DVec2::ZERO, radius: 10.0 }, Segment::Circle { center: DVec2::ZERO, radius: 4.0 }],
        0.0,
        2.0,
    )?;
    let v = volume(&washer.triangles(0.005)?);
    let exact = std::f64::consts::PI * (100.0 - 16.0) * 2.0;
    assert!((v - exact).abs() / exact < 0.01, "{v} vs {exact}");
    // A slot: two lines and two half circles, drawn in any order and direction.
    let slot = [
        Segment::Line { a: DVec2::new(0.0, -5.0), b: DVec2::new(20.0, -5.0) },
        Segment::Line { a: DVec2::new(0.0, 5.0), b: DVec2::new(20.0, 5.0) },
        Segment::Arc {
            center: DVec2::new(20.0, 0.0),
            radius: 5.0,
            start: -std::f64::consts::FRAC_PI_2,
            end: std::f64::consts::FRAC_PI_2,
            ccw: true,
        },
        Segment::Arc {
            center: DVec2::ZERO,
            radius: 5.0,
            start: std::f64::consts::FRAC_PI_2,
            end: -std::f64::consts::FRAC_PI_2,
            ccw: true,
        },
    ];
    let v = volume(&Body::extrude(&plane, &slot, 0.0, 3.0)?.triangles(0.005)?);
    let exact = (20.0 * 10.0 + std::f64::consts::PI * 25.0) * 3.0;
    assert!((v - exact).abs() / exact < 0.01, "{v} vs {exact}");
    // A 10 × 20 rectangle 5 off the Y axis, turned about it: a tube.
    let ring: Vec<Segment> = rect(10.0, 20.0)
        .into_iter()
        .map(|s| match s {
            Segment::Line { a, b } => Segment::Line { a: a + DVec2::new(5.0, 0.0), b: b + DVec2::new(5.0, 0.0) },
            o => o,
        })
        .collect();
    let tube = Body::revolve(&plane, &ring, DVec2::ZERO, DVec2::Y, 360.0)?;
    let v = volume(&tube.triangles(0.01)?);
    let exact = std::f64::consts::PI * (225.0 - 25.0) * 20.0;
    assert!((v - exact).abs() / exact < 0.01, "{v} vs {exact}");
    // Turned the other way, and on a plane facing the other way: still outward.
    let flipped = Plane::new(DVec3::ZERO, DVec3::X, DVec3::NEG_Y).ok_or_else(|| err("plane"))?;
    for (plane, angle) in [(plane, -360.0), (flipped, 360.0), (flipped, 90.0)] {
        let v = volume(&Body::revolve(&plane, &ring, DVec2::ZERO, DVec2::Y, angle)?.triangles(0.01)?);
        let want = exact * angle.abs() / 360.0;
        assert!((v - want).abs() / want < 0.01, "{v} vs {want} at {angle}°");
    }
    Ok(())
}

#[test]
fn bad_profiles_are_errors() {
    let plane = Plane { origin: DVec3::ZERO, x: DVec3::X, y: DVec3::Y };
    let open = [Segment::Line { a: DVec2::ZERO, b: DVec2::X }];
    assert!(Body::extrude(&plane, &open, 0.0, 1.0).is_err());
    assert!(Body::extrude(&plane, &rect(1.0, 1.0), 0.0, 0.0).is_err());
    assert!(Body::extrude(&plane, &[], 0.0, 1.0).is_err());
    assert!(Body::extrude(&plane, &[Segment::Circle { center: DVec2::ZERO, radius: 0.0 }], 0.0, 1.0).is_err());
    assert!(Body::revolve(&plane, &rect(1.0, 1.0), DVec2::ZERO, DVec2::ZERO, 90.0).is_err());
    assert!(Body::extrude(&plane, &rect(f64::NAN, 1.0), 0.0, 1.0).is_err());
}
