use quill_weave::{BRAND_FRAME, State, point_count, project};

const DOTS: [[u32; 4]; 2] = [[0x01, 0x02, 0x04, 0x40], [0x08, 0x10, 0x20, 0x80]];

pub fn mark(columns: usize, rows: usize, state: State, seconds: f64) -> Vec<String> {
    let (width, height) = (columns * 2, rows * 4);
    let side = width.min(height) as f64;
    let radius = side * 0.46;
    let mut grid = vec![vec![0u32; columns]; rows];
    let params = state.params();
    let mut clock = BRAND_FRAME;
    clock.advance(&params, seconds);
    let mut dots = Vec::new();
    project(point_count(side * 4.0), &params, &clock, &mut dots);
    for dot in dots.iter().filter(|dot| dot.alpha > 0.3) {
        let x = (width as f64 / 2.0 + dot.x * radius).floor();
        let y = (height as f64 / 2.0 + dot.y * radius).floor();
        if x < 0.0 || y < 0.0 || x >= width as f64 || y >= height as f64 {
            continue;
        }
        let (x, y) = (x as usize, y as usize);
        grid[y / 4][x / 2] |= DOTS[x % 2][y % 4];
    }
    grid.iter().map(|row| row.iter().map(|bits| char::from_u32(0x2800 + bits).unwrap_or(' ')).collect()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_mark_is_braille_of_the_asked_size() {
        let rows = mark(8, 4, State::Idle, 0.0);
        assert_eq!(rows.len(), 4);
        assert!(rows.iter().all(|row| row.chars().count() == 8));
        assert!(rows.iter().flat_map(|row| row.chars()).all(|c| ('\u{2800}'..='\u{28ff}').contains(&c)));
        assert!(rows.iter().flat_map(|row| row.chars()).any(|c| c != '\u{2800}'));
        assert_eq!(mark(8, 4, State::Idle, 0.0), rows);
    }

    #[test]
    fn the_activity_mark_is_one_line_that_moves() {
        for state in [State::Thinking, State::Generating] {
            let frames: Vec<String> = (0..12).map(|step| mark(2, 1, state, step as f64 * 0.08).concat()).collect();
            assert!(frames.iter().all(|frame| frame.chars().count() == 2));
            assert!(frames.windows(2).any(|pair| pair[0] != pair[1]), "{state:?} {frames:?}");
        }
    }

    #[test]
    fn the_mark_moves_over_time() {
        assert_ne!(mark(8, 4, State::Idle, 0.0), mark(8, 4, State::Idle, 2.0));
    }
}