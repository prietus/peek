use image::DynamicImage;

#[derive(Copy, Clone, Debug)]
pub(super) enum EditOp {
    RotateCw,
    RotateCcw,
    Rotate180,
    FlipH,
    FlipV,
}

impl EditOp {
    pub(super) fn apply(self, img: DynamicImage) -> DynamicImage {
        match self {
            EditOp::RotateCw => img.rotate90(),
            EditOp::RotateCcw => img.rotate270(),
            EditOp::Rotate180 => img.rotate180(),
            EditOp::FlipH => img.fliph(),
            EditOp::FlipV => img.flipv(),
        }
    }
}

pub(super) enum MenuItem {
    Op(&'static str, EditOp),
    Separator,
}

pub(super) const MENU: &[MenuItem] = &[
    MenuItem::Op("Rotate 90° CW   (r)", EditOp::RotateCw),
    MenuItem::Op("Rotate 90° CCW  (R)", EditOp::RotateCcw),
    MenuItem::Op("Rotate 180°", EditOp::Rotate180),
    MenuItem::Separator,
    MenuItem::Op("Flip horizontal (f)", EditOp::FlipH),
    MenuItem::Op("Flip vertical   (F)", EditOp::FlipV),
];

#[derive(Copy, Clone, Debug)]
pub(super) struct CropState {
    /// Selection corners as fractions of the image (0.0 .. 1.0). x0/y0 is the
    /// top-left, x1/y1 the bottom-right; kept ordered so apply_crop doesn't
    /// have to care which side moved.
    pub(super) x0: f32,
    pub(super) y0: f32,
    pub(super) x1: f32,
    pub(super) y1: f32,
}

impl CropState {
    pub(super) fn new() -> Self {
        Self {
            x0: 0.25,
            y0: 0.25,
            x1: 0.75,
            y1: 0.75,
        }
    }

    /// Selection in pixels as (x, y, width, height), clamped so it always
    /// lies inside a `w × h` image and is at least 1×1.
    pub(super) fn pixel_rect(&self, w: u32, h: u32) -> (u32, u32, u32, u32) {
        let px = |f: f32, n: u32| ((f.clamp(0.0, 1.0) * n as f32).round() as u32).min(n.saturating_sub(1));
        let x0 = px(self.x0.min(self.x1), w);
        let y0 = px(self.y0.min(self.y1), h);
        let x1 = (self.x0.max(self.x1).clamp(0.0, 1.0) * w as f32).round() as u32;
        let y1 = (self.y0.max(self.y1).clamp(0.0, 1.0) * h as f32).round() as u32;
        let cw = x1.saturating_sub(x0).max(1).min(w - x0);
        let ch = y1.saturating_sub(y0).max(1).min(h - y0);
        (x0, y0, cw, ch)
    }
}

pub(super) fn first_op_index() -> usize {
    MENU.iter()
        .position(|item| matches!(item, MenuItem::Op(_, _)))
        .unwrap_or(0)
}

pub(super) fn next_op_index(current: usize, delta: i32) -> usize {
    let len = MENU.len() as i32;
    let mut i = current as i32;
    loop {
        i += delta;
        if i < 0 {
            i = len - 1;
        } else if i >= len {
            i = 0;
        }
        if let MenuItem::Op(_, _) = MENU[i as usize] {
            return i as usize;
        }
        if i == current as i32 {
            return current;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crop_rect_default_is_centered_half() {
        assert_eq!(CropState::new().pixel_rect(100, 200), (25, 50, 50, 100));
    }

    #[test]
    fn crop_rect_full_and_degenerate() {
        let full = CropState { x0: 0.0, y0: 0.0, x1: 1.0, y1: 1.0 };
        assert_eq!(full.pixel_rect(640, 480), (0, 0, 640, 480));
        // Swapped / zero-size selections still yield a valid 1×1 rect.
        let flat = CropState { x0: 1.0, y0: 1.0, x1: 1.0, y1: 1.0 };
        let (x, y, w, h) = flat.pixel_rect(10, 10);
        assert!(x + w <= 10 && y + h <= 10 && w >= 1 && h >= 1);
    }

    #[test]
    fn menu_navigation_skips_separators() {
        let first = first_op_index();
        assert!(matches!(MENU[first], MenuItem::Op(..)));
        for i in 0..MENU.len() {
            if matches!(MENU[i], MenuItem::Op(..)) {
                assert!(matches!(MENU[next_op_index(i, 1)], MenuItem::Op(..)));
                assert!(matches!(MENU[next_op_index(i, -1)], MenuItem::Op(..)));
            }
        }
        // Wraps around from the last op to the first.
        let last = MENU.iter().rposition(|m| matches!(m, MenuItem::Op(..))).unwrap();
        assert_eq!(next_op_index(last, 1), first);
    }
}
