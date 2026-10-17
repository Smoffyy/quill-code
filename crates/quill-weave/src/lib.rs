use std::f64::consts::TAU;

const GOLDEN: f64 = 2.399_963_229_728_653;
const PERSPECTIVE: f64 = 3.4;
const STRANDS: usize = 3;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum State {
    Idle,
    Thinking,
    Generating,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Params {
    pub braid: f64,
    pub ribbon: f64,
    pub glass: f64,
    pub spin: f64,
    pub stream: f64,
}

impl State {
    pub fn params(self) -> Params {
        match self {
            State::Idle => Params { braid: 1.0, ribbon: 0.0, glass: 0.0, spin: 0.06, stream: 0.0 },
            State::Thinking => Params { braid: 0.0, ribbon: 0.0, glass: 1.0, spin: 0.0, stream: 0.0 },
            State::Generating => Params { braid: 0.0, ribbon: 1.0, glass: 0.0, spin: 0.0, stream: 0.55 },
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Clock {
    pub time: f64,
    pub angle: f64,
    pub flow: f64,
}

pub const BRAND_FRAME: Clock = Clock { time: 123.1833, angle: 0.5074, flow: 0.0 };

impl Clock {
    pub fn advance(&mut self, params: &Params, dt: f64) {
        self.angle += params.spin * dt;
        self.flow += params.stream * dt;
        self.time += dt;
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Dot {
    pub x: f64,
    pub y: f64,
    pub depth: f64,
    pub alpha: f64,
}

pub fn point_count(px: f64) -> usize {
    STRANDS * (px * 1.5).round().clamp(20.0, 110.0) as usize
}

#[derive(Clone, Copy, Default)]
struct Sample {
    x: f64,
    y: f64,
    z: f64,
    depth: f64,
    fade: f64,
    hot: f64,
}

struct View {
    cy: f64,
    sy: f64,
    cx: f64,
    sx: f64,
}

fn frac(v: f64) -> f64 {
    v - v.floor()
}

fn hash(v: f64) -> f64 {
    frac((v * 12.9898 + 4.1414).sin() * 43758.5453)
}

fn pose(x: f64, y: f64, z: f64, view: &View) -> Sample {
    let x1 = x * view.cy + z * view.sy;
    let z1 = -x * view.sy + z * view.cy;
    let pz = y * view.sx + z1 * view.cx;
    Sample { x: x1, y: y * view.cx - z1 * view.sx, z: pz, depth: ((pz + 1.0) / 2.0).clamp(0.0, 1.0), ..Sample::default() }
}

fn braid_at(i: usize, time: f64, view: &View) -> Sample {
    let i = i as f64;
    let h1 = hash(i + 0.1);
    let h2 = hash(i * 1.7 + 0.2);
    let h3 = hash(i * 2.3 + 0.3);
    let th = h1 * TAU;
    let cz = h2 * 2.0 - 1.0;
    let sq = (1.0 - cz * cz).sqrt();
    let rr = 0.72 * h3.cbrt();
    let x = rr * sq * th.cos() + 0.1 * (time * 0.5 + h1 * 9.0).sin();
    let y = rr * cz + 0.1 * (time * 0.43 + h2 * 9.0).sin();
    let z = rr * sq * th.sin() + 0.1 * (time * 0.37 + h3 * 9.0).sin();
    let tw = 0.5 + 0.5 * (time * (0.7 + h2) + h3 * 20.0).sin();
    Sample { fade: 0.25 + 0.75 * tw.powi(3), hot: tw.powi(12), ..pose(x, y, z, view) }
}

fn ribbon_at(u: f64, strand: usize, j: usize, flow: f64) -> Sample {
    let base = strand as f64 * (TAU / STRANDS as f64) + flow * TAU;
    let th = j as f64 * GOLDEN;
    let rr = 0.05 * u.sqrt();
    Sample { x: 0.55 * base.cos() + rr * th.cos(), y: 0.55 * base.sin() + rr * th.sin(), z: 0.0, depth: 0.85, fade: 1.0, hot: 0.6 }
}

fn glass_at(u: f64, strand: usize, j: usize, time: f64) -> Sample {
    let bounce = (time * 3.0 - strand as f64 * 0.9).sin().max(0.0);
    let th = j as f64 * GOLDEN;
    let rr = 0.09 * u.sqrt();
    Sample {
        x: (strand as f64 - 1.0) * 0.55 + rr * th.cos(),
        y: -0.15 + bounce * 0.28 + rr * th.sin(),
        z: 0.0,
        depth: 0.85,
        fade: 1.0,
        hot: bounce * 0.6,
    }
}

pub fn project(n: usize, params: &Params, clock: &Clock, out: &mut Vec<Dot>) {
    let per = n.div_ceil(STRANDS).max(1);
    let rx = 0.35 + 0.18 * (clock.time * 0.27).sin();
    let view = View { cy: clock.angle.cos(), sy: clock.angle.sin(), cx: rx.cos(), sx: rx.sin() };
    let weights = [params.braid, params.ribbon, params.glass];
    let total: f64 = weights.iter().filter(|w| **w > 1e-4).sum();
    out.clear();
    for i in 0..n {
        let strand = i % STRANDS;
        let j = i / STRANDS;
        let u = j as f64 / per as f64;
        let mut acc = Sample::default();
        for (form, weight) in weights.iter().enumerate() {
            if *weight <= 1e-4 {
                continue;
            }
            let s = match form {
                0 => braid_at(i, clock.time, &view),
                1 => ribbon_at(u, strand, j, clock.flow),
                _ => glass_at(u, strand, j, clock.time),
            };
            let w = weight / total;
            acc.x += s.x * w;
            acc.y += s.y * w;
            acc.z += s.z * w;
            acc.depth += s.depth * w;
            acc.hot += s.hot * w;
            acc.fade += s.fade * w;
        }
        let persp = PERSPECTIVE / (PERSPECTIVE - acc.z);
        let glow = acc.hot.min(1.0) * (0.4 + 0.6 * acc.depth);
        out.push(Dot {
            x: acc.x * persp,
            y: -acc.y * persp,
            depth: acc.depth,
            alpha: (0.16 + 0.84 * acc.depth.powf(1.4) + glow * 0.3).min(1.0) * acc.fade,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn point_count_follows_size_within_bounds() {
        assert_eq!(point_count(4.0), 60);
        assert_eq!(point_count(40.0), 180);
        assert_eq!(point_count(1000.0), 330);
    }

    #[test]
    fn projection_is_finite_and_bounded() {
        let mut dots = Vec::new();
        for state in [State::Idle, State::Thinking, State::Generating] {
            project(150, &state.params(), &BRAND_FRAME, &mut dots);
            assert_eq!(dots.len(), 150);
            for d in &dots {
                assert!(d.x.is_finite() && d.y.is_finite());
                assert!(d.x.abs() < 1.6 && d.y.abs() < 1.6);
                assert!((0.0..=1.0).contains(&d.alpha));
                assert!((0.0..=1.0).contains(&d.depth));
            }
        }
    }

    #[test]
    fn brand_frame_is_deterministic() {
        let mut a = Vec::new();
        let mut b = Vec::new();
        project(120, &State::Idle.params(), &BRAND_FRAME, &mut a);
        project(120, &State::Idle.params(), &BRAND_FRAME, &mut b);
        assert_eq!(a, b);
    }

    #[test]
    fn hash_matches_javascript() {
        assert!((hash(0.1) - 0.693_451_402_992_650_4).abs() < 1e-9);
    }

    #[test]
    fn brand_frame_matches_javascript() {
        let mut dots = Vec::new();
        project(120, &State::Idle.params(), &BRAND_FRAME, &mut dots);
        let expect = [
            (0, -0.170_752_602_060_309_26, 0.047_539_664_424_522_95, 0.496_013_871_497_697_05, 0.218_910_156_706_403_93),
            (57, 0.068_808_926_260_039_14, -0.009_501_988_326_992_393, 0.693_343_779_674_705_4, 0.906_447_357_569_037_9),
            (119, 0.459_069_969_060_720_2, -0.421_580_264_877_707_33, 0.434_048_337_762_537_76, 0.435_187_273_191_563_7),
        ];
        for (i, x, y, depth, alpha) in expect {
            let d = dots[i];
            assert!((d.x - x).abs() < 1e-9, "x {i}");
            assert!((d.y - y).abs() < 1e-9, "y {i}");
            assert!((d.depth - depth).abs() < 1e-9, "depth {i}");
            assert!((d.alpha - alpha).abs() < 1e-9, "alpha {i}");
        }
    }
}