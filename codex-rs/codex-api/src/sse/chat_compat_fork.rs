#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ChatReasoningFormat {
    Standard,
    MinimaxThinkTags,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ContentSegment {
    Assistant(String),
    Reasoning(String),
}

#[derive(Debug)]
pub(crate) struct ThinkTagStreamSplitter {
    format: ChatReasoningFormat,
    pending: String,
    in_think_block: bool,
}

impl ThinkTagStreamSplitter {
    pub(crate) fn new(format: ChatReasoningFormat) -> Self {
        Self {
            format,
            pending: String::new(),
            in_think_block: false,
        }
    }

    pub(crate) fn split_chunk(&mut self, text: &str) -> Vec<ContentSegment> {
        if text.is_empty() {
            return Vec::new();
        }

        match self.format {
            ChatReasoningFormat::Standard => {
                vec![ContentSegment::Assistant(text.to_string())]
            }
            ChatReasoningFormat::MinimaxThinkTags => self.split_minimax_chunk(text),
        }
    }

    pub(crate) fn flush_remaining(&mut self) -> Vec<ContentSegment> {
        if self.pending.is_empty() {
            return Vec::new();
        }

        let pending = std::mem::take(&mut self.pending);
        if self.in_think_block {
            vec![ContentSegment::Reasoning(pending)]
        } else {
            vec![ContentSegment::Assistant(pending)]
        }
    }

    fn split_minimax_chunk(&mut self, text: &str) -> Vec<ContentSegment> {
        const OPEN_TAG: &str = "<think>";
        const CLOSE_TAG: &str = "</think>";

        self.pending.push_str(text);
        let mut segments = Vec::new();

        loop {
            if self.in_think_block {
                if let Some(pos) = self.pending.find(CLOSE_TAG) {
                    if pos > 0 {
                        segments.push(ContentSegment::Reasoning(take_prefix(
                            &mut self.pending,
                            pos,
                        )));
                    }
                    self.pending.drain(..CLOSE_TAG.len());
                    self.in_think_block = false;
                    continue;
                }

                let keep = trailing_partial_tag_len(&self.pending, CLOSE_TAG);
                let emit_len = self.pending.len().saturating_sub(keep);
                if emit_len > 0 {
                    segments.push(ContentSegment::Reasoning(take_prefix(
                        &mut self.pending,
                        emit_len,
                    )));
                }
                break;
            }

            if let Some(pos) = self.pending.find(OPEN_TAG) {
                if pos > 0 {
                    segments.push(ContentSegment::Assistant(take_prefix(
                        &mut self.pending,
                        pos,
                    )));
                }
                self.pending.drain(..OPEN_TAG.len());
                self.in_think_block = true;
                continue;
            }

            let keep = trailing_partial_tag_len(&self.pending, OPEN_TAG);
            let emit_len = self.pending.len().saturating_sub(keep);
            if emit_len > 0 {
                segments.push(ContentSegment::Assistant(take_prefix(
                    &mut self.pending,
                    emit_len,
                )));
            }
            break;
        }

        segments
    }
}

fn trailing_partial_tag_len(buffer: &str, tag: &str) -> usize {
    let max = buffer.len().min(tag.len().saturating_sub(1));
    let bytes = buffer.as_bytes();
    let tag_bytes = tag.as_bytes();

    for len in (1..=max).rev() {
        if tag_bytes.starts_with(&bytes[bytes.len() - len..]) {
            return len;
        }
    }

    0
}

fn take_prefix(buffer: &mut String, end: usize) -> String {
    let tail = buffer.split_off(end);
    std::mem::replace(buffer, tail)
}

#[cfg(test)]
mod tests {
    use super::ChatReasoningFormat;
    use super::ContentSegment;
    use super::ThinkTagStreamSplitter;
    use pretty_assertions::assert_eq;

    #[test]
    fn splits_minimax_think_blocks() {
        let mut splitter = ThinkTagStreamSplitter::new(ChatReasoningFormat::MinimaxThinkTags);
        let segments = splitter.split_chunk("a<think>b</think>c");
        assert_eq!(
            segments,
            vec![
                ContentSegment::Assistant("a".to_string()),
                ContentSegment::Reasoning("b".to_string()),
                ContentSegment::Assistant("c".to_string())
            ]
        );
    }

    #[test]
    fn handles_split_tags_across_chunks() {
        let mut splitter = ThinkTagStreamSplitter::new(ChatReasoningFormat::MinimaxThinkTags);
        let mut segments = splitter.split_chunk("<th");
        assert_eq!(segments, vec![]);

        segments.extend(splitter.split_chunk("ink>abc</thi"));
        segments.extend(splitter.split_chunk("nk>done"));
        assert_eq!(
            segments,
            vec![
                ContentSegment::Reasoning("abc".to_string()),
                ContentSegment::Assistant("done".to_string())
            ]
        );
    }

    #[test]
    fn standard_mode_passthrough() {
        let mut splitter = ThinkTagStreamSplitter::new(ChatReasoningFormat::Standard);
        assert_eq!(
            splitter.split_chunk("<think>x</think>"),
            vec![ContentSegment::Assistant("<think>x</think>".to_string())]
        );
    }
}
