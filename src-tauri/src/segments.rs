//! 字幕段边界清洗
//!
//! STT/模型时间戳不是可信输入：可能出现 NaN、负数、end<=start、零长度段。
//! 这些坏段如果直接喂给 ffmpeg，会触发 `-to value smaller than -ss` 并中断全流程。
//! 所以所有 STT 输出必须先过这里，形成后续阶段的统一契约。

use crate::types::Segment;

const MIN_DUR: f64 = 0.02;

pub fn sanitize(mut segments: Vec<Segment>) -> Vec<Segment> {
    segments.sort_by(|a, b| {
        a.start
            .partial_cmp(&b.start)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    let mut out = Vec::with_capacity(segments.len());
    for mut s in segments {
        s.text = s.text.trim().to_string();
        if s.text.is_empty() {
            log::warn!("drop segment {}: empty text", s.idx);
            continue;
        }
        if !s.start.is_finite() || !s.end.is_finite() {
            log::warn!(
                "drop segment {}: non-finite time start={:?} end={:?}",
                s.idx,
                s.start,
                s.end
            );
            continue;
        }
        if s.end <= s.start + MIN_DUR {
            log::warn!(
                "drop segment {}: invalid time start={:.3} end={:.3}",
                s.idx,
                s.start,
                s.end
            );
            continue;
        }
        if s.start < 0.0 {
            log::warn!("clamp segment {} start from {:.3} to 0", s.idx, s.start);
            s.start = 0.0;
        }
        s.idx = out.len();
        out.push(s);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seg(idx: usize, start: f64, end: f64) -> Segment {
        Segment {
            idx,
            start,
            end,
            text: format!("s{idx}"),
            translated: String::new(),
        }
    }

    #[test]
    fn drops_zero_and_reversed_segments_and_reindexes() {
        let out = sanitize(vec![
            seg(10, 0.0, 1.0),
            seg(11, 2.0, 1.0),
            seg(12, 3.0, 3.0),
        ]);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].idx, 0);
        assert_eq!(out[0].text, "s10");
    }

    #[test]
    fn drops_empty_text_segments() {
        let mut blank = seg(0, 0.0, 1.0);
        blank.text = "   ".into();
        let mut kept = seg(1, 1.0, 2.0);
        kept.text = "  hello  ".into();
        let out = sanitize(vec![blank, kept]);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].idx, 0);
        assert_eq!(out[0].text, "hello");
    }

    #[test]
    fn drops_nan_segments() {
        let out = sanitize(vec![seg(0, f64::NAN, 1.0), seg(1, 1.0, f64::INFINITY)]);
        assert!(out.is_empty());
    }

    #[test]
    fn clamps_negative_start_and_sorts() {
        let out = sanitize(vec![seg(1, 5.0, 6.0), seg(0, -0.5, 1.0)]);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].idx, 0);
        assert_eq!(out[0].start, 0.0);
        assert_eq!(out[1].idx, 1);
        assert_eq!(out[1].start, 5.0);
    }
}
