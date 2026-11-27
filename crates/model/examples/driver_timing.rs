//! How long driven pegs take to place (plan §8.8): a car keyed over a shot of 1000 frames, with
//! four wheels whose turn is worked out from the way it has travelled, and a peg on each wheel
//! that carries a drawing. Every frame, each drawing's world is worked out once, as the
//! evaluator does.
//!
//! `cargo run --release -p iskra-model --example driver_timing`

use std::sync::Arc;
use std::time::Instant;

use iskra_model::{Animation, Driver, DriverId, DriverParameter, Expressions, Peg, PegId, Project, Track, Transform};

fn peg(id: PegId, name: &str, parent: Option<PegId>) -> Peg {
    Peg {
        id,
        name: name.to_string(),
        parent,
        transform: Transform::default(),
        animation: Animation::default(),
        template: None,
        follows: None,
        inherit: Default::default(),
        matches: Default::default(),
    }
}

fn main() {
    const FRAMES: u32 = 1000;
    for driven in [false, true] {
        let mut project = Project::default();
        let car = PegId(project.new_id());
        let mut car_peg = peg(car, "Car-P", None);
        car_peg.animation.position.set(0, [0.0, 0.0]);
        car_peg.animation.position.set(FRAMES / 2, [2000.0, -300.0]);
        car_peg.animation.position.set(FRAMES, [0.0, 0.0]);
        let mut pegs = vec![Arc::new(car_peg)];
        let mut drivers = Vec::new();
        let mut hubs = Vec::new();
        for i in 0..4 {
            let (wheel, hub) = (PegId(project.new_id()), PegId(project.new_id()));
            pegs.push(Arc::new(peg(wheel, &format!("Wheel-{i}"), Some(car))));
            pegs.push(Arc::new(peg(hub, &format!("Hub-{i}"), Some(wheel))));
            hubs.push(hub);
            let mut driver = Driver::new(DriverId(project.new_id()), &format!("Driver {i}"), wheel);
            driver.expressions = Expressions::parse("rotation = parent.travelled / radius * 180 / PI").unwrap();
            driver.parameters = vec![DriverParameter { name: "radius".into(), value: 40.0, track: Track::default() }];
            drivers.push(Arc::new(driver));
        }
        let shot = Arc::make_mut(&mut project.shots[0]);
        shot.pegs = pegs;
        if driven {
            shot.drivers = drivers;
        }
        let shot = &project.shots[0];
        let started = Instant::now();
        let mut sum = 0.0;
        for frame in 0..FRAMES {
            for hub in &hubs {
                sum += shot.world(Some(*hub), frame).m[0];
            }
        }
        let took = started.elapsed();
        // Right after a change, nothing is kept: the last frame worked out cold, on a thread
        // of its own.
        let cold = std::thread::scope(|scope| {
            scope
                .spawn(|| {
                    let started = Instant::now();
                    let _ = shot.world(Some(hubs[0]), FRAMES - 1);
                    started.elapsed()
                })
                .join()
                .unwrap()
        });
        println!("  cold, one hub on the last frame: {:.3} ms", cold.as_secs_f64() * 1e3);
        let last = Instant::now();
        for hub in &hubs {
            sum += shot.world(Some(*hub), FRAMES - 1).m[0];
        }
        println!(
            "{}: {FRAMES} frames, 4 hubs: {:.2} ms in all, {:.3} ms a frame on average, {:.3} ms on the last frame ({sum:.1})",
            if driven { "with drivers" } else { "without" },
            took.as_secs_f64() * 1e3,
            took.as_secs_f64() * 1e3 / f64::from(FRAMES),
            last.elapsed().as_secs_f64() * 1e3,
        );
    }
}
