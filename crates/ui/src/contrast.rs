//! Contrast helpers (WCAG relative luminance): pick readable text for a bar and a highlight
//! outline that stands out on any tile colour. Pure functions over token colours.

use crate::theme::{Rgba, Tokens};

/// WCAG relative luminance of an sRGB colour (alpha ignored), `0.0..=1.0`.
pub fn luminance(c: Rgba) -> f32 {
    let channel = |v: u8| {
        let s = f32::from(v) / 255.0;
        if s <= 0.039_28 { s / 12.92 } else { ((s + 0.055) / 1.055).powf(2.4) }
    };
    0.2126 * channel(c[0]) + 0.7152 * channel(c[1]) + 0.0722 * channel(c[2])
}

/// WCAG contrast ratio of two colours, `1.0..=21.0`.
pub fn contrast_ratio(a: Rgba, b: Rgba) -> f32 {
    let (la, lb) = (luminance(a), luminance(b));
    let (hi, lo) = if la >= lb { (la, lb) } else { (lb, la) };
    (hi + 0.05) / (lo + 0.05)
}

/// The candidate whose worst contrast against every background is the highest. With no candidates
/// the result is opaque black; with no backgrounds the first candidate.
pub fn best_on(candidates: &[Rgba], backgrounds: &[Rgba]) -> Rgba {
    let worst = |c: Rgba| backgrounds.iter().map(|b| contrast_ratio(c, *b)).fold(f32::INFINITY, f32::min);
    candidates
        .iter()
        .copied()
        .fold(None::<(Rgba, f32)>, |best, c| {
            let score = worst(c);
            match best {
                Some((_, s)) if s >= score => best,
                _ => Some((c, score)),
            }
        })
        .map_or([0, 0, 0, 255], |(c, _)| c)
}

/// The backgrounds a label spanning `label_from..label_to` sits on in a bar whose fill ends at
/// `fill_to` (all x positions in the same space): the fill, the track, or both.
pub fn bar_backgrounds(label_from: f32, label_to: f32, fill_to: f32, fill: Rgba, track: Rgba) -> Vec<Rgba> {
    let mut out = Vec::with_capacity(2);
    if label_from < fill_to {
        out.push(fill);
    }
    if label_to > fill_to || out.is_empty() {
        out.push(track);
    }
    out
}

/// Text colour for a label drawn over `backgrounds` (a bar fill, a track or both).
pub fn text_on(tokens: &Tokens, backgrounds: &[Rgba]) -> egui::Color32 {
    Tokens::color(best_on(&[tokens.text, tokens.background], backgrounds))
}

/// Inner and outer colour of a search-hit outline on a tile of colour `tile`: the two theme
/// extremes (text and background), ordered so the inner ring contrasts best with the tile.
pub fn outline_pair(tokens: &Tokens, tile: Rgba) -> (egui::Color32, egui::Color32) {
    let inner = best_on(&[tokens.text, tokens.background], &[tile]);
    let outer = if inner == tokens.text { tokens.background } else { tokens.text };
    (Tokens::color(inner), Tokens::color(outer))
}

#[cfg(test)]
mod tests {
    use super::*;

    const BLACK: Rgba = [0, 0, 0, 255];
    const WHITE: Rgba = [255, 255, 255, 255];

    #[test]
    fn ratios_match_the_wcag_extremes() {
        assert!((contrast_ratio(BLACK, WHITE) - 21.0).abs() < 0.01);
        assert!((contrast_ratio(WHITE, WHITE) - 1.0).abs() < 0.001);
        assert_eq!(contrast_ratio(BLACK, WHITE), contrast_ratio(WHITE, BLACK));
    }

    #[test]
    fn picks_the_readable_text_for_dark_and_light_bars_in_both_themes() {
        for tokens in [Tokens::dark(), Tokens::light()] {
            let dark_bar: Rgba = [20, 60, 140, 255];
            let light_bar: Rgba = [250, 235, 150, 255];
            for bar in [dark_bar, light_bar] {
                let picked = best_on(&[tokens.text, tokens.background], &[bar]);
                let other = if picked == tokens.text { tokens.background } else { tokens.text };
                assert!(contrast_ratio(picked, bar) >= contrast_ratio(other, bar));
            }
            let on_dark = best_on(&[tokens.text, tokens.background], &[dark_bar]);
            let on_light = best_on(&[tokens.text, tokens.background], &[light_bar]);
            assert!(luminance(on_dark) > luminance(on_light), "light text on the dark bar, dark on the light one");
            assert!(contrast_ratio(on_dark, dark_bar) >= 4.5);
            assert!(contrast_ratio(on_light, light_bar) >= 4.5);
        }
    }

    #[test]
    fn bar_labels_sit_on_the_fill_the_track_or_both() {
        let (fill, track) = ([20, 60, 140, 255], [200, 200, 200, 255]);
        assert_eq!(bar_backgrounds(10.0, 30.0, 50.0, fill, track), vec![fill]);
        assert_eq!(bar_backgrounds(60.0, 80.0, 50.0, fill, track), vec![track]);
        assert_eq!(bar_backgrounds(40.0, 60.0, 50.0, fill, track), vec![fill, track]);
        assert_eq!(bar_backgrounds(40.0, 60.0, 0.0, fill, track), vec![track]);
        assert_eq!(bar_backgrounds(f32::NAN, f32::NAN, f32::NAN, fill, track), vec![track]);
    }

    #[test]
    fn a_label_over_two_backgrounds_must_work_on_both() {
        let tokens = Tokens::light();
        let dark: Rgba = [30, 30, 30, 255];
        let light: Rgba = [240, 240, 240, 255];
        let picked = best_on(&[BLACK, WHITE], &[dark, light]);
        let worst = contrast_ratio(picked, dark).min(contrast_ratio(picked, light));
        let alt = if picked == BLACK { WHITE } else { BLACK };
        assert!(worst >= contrast_ratio(alt, dark).min(contrast_ratio(alt, light)));
        // Degenerate inputs never panic.
        assert_eq!(best_on(&[], &[dark]), [0, 0, 0, 255]);
        assert_eq!(best_on(&[WHITE], &[]), WHITE);
        let _ = text_on(&tokens, &[]);
    }

    #[test]
    fn outline_rings_differ_and_contrast_with_any_tile() {
        for tokens in [Tokens::dark(), Tokens::light()] {
            for tile in [[245, 130, 48, 255], [20, 20, 20, 255], [240, 240, 240, 255], [128, 128, 128, 255]] {
                let (inner, outer) = outline_pair(&tokens, tile);
                assert_ne!(inner, outer);
                let inner_rgba = [inner.r(), inner.g(), inner.b(), 255];
                assert!(contrast_ratio(inner_rgba, tile) >= 3.0, "{tile:?}");
            }
        }
    }
}
