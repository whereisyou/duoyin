use crate::types::Segment;

pub fn align_text_to_segments(segments: &[Segment], text: &str) -> Result<Vec<Segment>, String> {
    if segments.is_empty() {
        return Err("没有字幕时间轴".into());
    }
    let lines: Vec<_> = text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect();
    if lines.is_empty() {
        return Err("匹配文本为空".into());
    }
    let mut output = segments.to_vec();
    if lines.len() == output.len() {
        for (segment, line) in output.iter_mut().zip(lines) {
            segment.translated = line.into();
        }
        return Ok(output);
    }
    let normalized = lines.join(" ");
    let chars: Vec<char> = normalized.chars().collect();
    let total_weight: usize = segments
        .iter()
        .map(|segment| segment.text.chars().count().max(1))
        .sum();
    let mut cursor = 0usize;
    for (index, segment) in output.iter_mut().enumerate() {
        let end = if index + 1 == segments.len() {
            chars.len()
        } else {
            let weight: usize = segments[..=index]
                .iter()
                .map(|item| item.text.chars().count().max(1))
                .sum();
            ((chars.len() as f64 * weight as f64 / total_weight as f64).round() as usize)
                .clamp(cursor, chars.len())
        };
        segment.translated = chars[cursor..end]
            .iter()
            .collect::<String>()
            .trim()
            .to_owned();
        cursor = end;
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn equal_lines_map_directly() {
        let source = vec![
            Segment {
                idx: 0,
                start: 0.0,
                end: 1.0,
                text: "a".into(),
                translated: String::new(),
            },
            Segment {
                idx: 1,
                start: 1.0,
                end: 2.0,
                text: "b".into(),
                translated: String::new(),
            },
        ];
        let output = align_text_to_segments(&source, "hello\nworld").unwrap();
        assert_eq!(output[0].translated, "hello");
        assert_eq!(output[1].translated, "world");
    }

    #[test]
    fn mismatched_lines_use_source_text_weights() {
        let source = vec![
            Segment {
                idx: 0,
                start: 0.0,
                end: 1.0,
                text: "a".into(),
                translated: String::new(),
            },
            Segment {
                idx: 1,
                start: 1.0,
                end: 2.0,
                text: "bbb".into(),
                translated: String::new(),
            },
        ];
        let output = align_text_to_segments(&source, "abcdefgh").unwrap();
        assert!(output[0].translated.len() < output[1].translated.len());
    }
}
