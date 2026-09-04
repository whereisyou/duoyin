use crate::types::Segment;

pub fn parse_srt(content: &str) -> Result<Vec<Segment>, String> {
    let normalized = content.replace("\r\n", "\n");
    let mut segments = Vec::new();
    for block in normalized
        .split("\n\n")
        .filter(|block| !block.trim().is_empty())
    {
        let mut lines = block.lines();
        let index_line = lines.next().ok_or("SRT 缺少序号")?.trim();
        let _number: usize = index_line
            .parse()
            .map_err(|_| format!("SRT 序号无效: {index_line}"))?;
        let timing = lines.next().ok_or("SRT 缺少时间轴")?;
        let (start, end) = timing
            .split_once("-->")
            .ok_or_else(|| format!("SRT 时间轴无效: {timing}"))?;
        let translated = lines.collect::<Vec<_>>().join("\n").trim().to_owned();
        if translated.is_empty() {
            return Err(format!("SRT 第 {} 段文本为空", segments.len() + 1));
        }
        segments.push(Segment {
            idx: segments.len(),
            start: parse_time(start.trim())?,
            end: parse_time(end.trim())?,
            text: String::new(),
            translated,
        });
    }
    if segments.is_empty() {
        return Err("SRT 没有字幕段".into());
    }
    if segments.iter().any(|segment| segment.end <= segment.start) {
        return Err("SRT 存在结束时间不大于开始时间的字幕段".into());
    }
    Ok(segments)
}

fn parse_time(value: &str) -> Result<f64, String> {
    let parts: Vec<_> = value
        .replace(',', ".")
        .split(':')
        .map(str::to_owned)
        .collect();
    if parts.len() != 3 {
        return Err(format!("SRT 时间无效: {value}"));
    }
    let hours: f64 = parts[0]
        .parse()
        .map_err(|_| format!("SRT 时间无效: {value}"))?;
    let minutes: f64 = parts[1]
        .parse()
        .map_err(|_| format!("SRT 时间无效: {value}"))?;
    let seconds: f64 = parts[2]
        .parse()
        .map_err(|_| format!("SRT 时间无效: {value}"))?;
    Ok(hours * 3600.0 + minutes * 60.0 + seconds)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_multiline_srt() {
        let segments = parse_srt(
            "1\n00:00:00,000 --> 00:00:01,500\nHello\nworld\n\n2\n00:00:02,000 --> 00:00:03,000\nBye\n",
        )
        .unwrap();
        assert_eq!(segments.len(), 2);
        assert_eq!(segments[0].translated, "Hello\nworld");
        assert_eq!(segments[0].end, 1.5);
    }
}
