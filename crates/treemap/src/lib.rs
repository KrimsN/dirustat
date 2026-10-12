//! L0: раскладка treemap (squarified, Bruls/Huizing/van Wijk). Чистая геометрия без зависимостей.
//!
//! Вход — веса по убыванию и прямоугольник, выход — по прямоугольнику на вес. Вес 0 даёт
//! прямоугольник нулевой площади. Функция не паникует ни на каких входах (NaN, пустой bounds).

/// Прямоугольник в пикселях/условных единицах.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub fn area(&self) -> f32 {
        self.w * self.h
    }
}

#[allow(clippy::cast_precision_loss)]
fn weight_to_f64(w: u64) -> f64 {
    // Потеря точности на огромных весах допустима: раскладка визуальная.
    w as f64
}

fn at(values: &[f64], i: usize) -> f64 {
    values.get(i).copied().unwrap_or(0.0)
}

/// Худшее отношение сторон в ряду: `max` и `min` — площади крайних элементов ряда.
fn worst_ratio(sum: f64, max: f64, min: f64, side: f64) -> f64 {
    if sum <= 0.0 || min <= 0.0 {
        return f64::INFINITY;
    }
    let s2 = side * side;
    (s2 * max / (sum * sum)).max(sum * sum / (s2 * min))
}

/// Раскладывает `weights` (по убыванию) в `bounds`. Результат имеет ту же длину, что и `weights`.
pub fn squarify(weights: &[u64], bounds: Rect) -> Vec<Rect> {
    let n = weights.len();
    let mut out = vec![Rect::default(); n];
    let total: f64 = weights.iter().map(|&w| weight_to_f64(w)).sum();
    let (bw, bh) = (f64::from(bounds.w), f64::from(bounds.h));
    if n == 0 || total <= 0.0 || !total.is_finite() || !(bw > 0.0 && bh > 0.0) {
        return out;
    }
    let scale = bw * bh / total;
    let areas: Vec<f64> = weights.iter().map(|&w| weight_to_f64(w) * scale).collect();
    let (mut rx, mut ry, mut rw, mut rh) = (f64::from(bounds.x), f64::from(bounds.y), bw, bh);

    let mut i = 0;
    while i < n && rw > 0.0 && rh > 0.0 {
        let side = rw.min(rh);
        let (mut j, mut sum, mut best) = (i, 0.0, f64::INFINITY);
        while j < n {
            let new_sum = sum + at(&areas, j);
            let worst = worst_ratio(new_sum, at(&areas, i), at(&areas, j), side);
            if j > i && worst > best {
                break;
            }
            best = worst;
            sum = new_sum;
            j += 1;
        }
        if sum <= 0.0 {
            break; // остались только нулевые веса
        }
        if rw >= rh {
            // колонка слева шириной sum / rh
            let cw = sum / rh;
            let mut y = ry;
            for k in i..j {
                let h = at(&areas, k) / cw;
                if let Some(slot) = out.get_mut(k) {
                    *slot = to_rect(rx, y, cw, h);
                }
                y += h;
            }
            rx += cw;
            rw -= cw;
        } else {
            // ряд сверху высотой sum / rw
            let row_h = sum / rw;
            let mut x = rx;
            for k in i..j {
                let w = at(&areas, k) / row_h;
                if let Some(slot) = out.get_mut(k) {
                    *slot = to_rect(x, ry, w, row_h);
                }
                x += w;
            }
            ry += row_h;
            rh -= row_h;
        }
        i = j;
    }
    out
}

#[allow(clippy::cast_possible_truncation)]
fn to_rect(x: f64, y: f64, w: f64, h: f64) -> Rect {
    Rect { x: x as f32, y: y as f32, w: w as f32, h: h as f32 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn areas_are_proportional_and_fill_bounds() {
        let weights = [60, 30, 6, 3, 1];
        let bounds = Rect { x: 0.0, y: 0.0, w: 400.0, h: 300.0 };
        let rects = squarify(&weights, bounds);
        let total: f32 = rects.iter().map(Rect::area).sum();
        assert!((total - bounds.area()).abs() < 1.0, "sum of areas {total}");
        assert!((rects[0].area() / bounds.area() - 0.6).abs() < 0.01);
    }

    #[test]
    fn degenerate_inputs_do_not_panic() {
        let b = Rect { x: 0.0, y: 0.0, w: 10.0, h: 10.0 };
        assert!(squarify(&[], b).is_empty());
        assert_eq!(squarify(&[0, 0], b), vec![Rect::default(); 2]);
        assert_eq!(squarify(&[1, 2], Rect::default()), vec![Rect::default(); 2]);
        let nan = Rect { x: 0.0, y: 0.0, w: f32::NAN, h: 5.0 };
        assert_eq!(squarify(&[1], nan).len(), 1);
        assert_eq!(squarify(&[u64::MAX, u64::MAX], b).len(), 2);
    }
}
