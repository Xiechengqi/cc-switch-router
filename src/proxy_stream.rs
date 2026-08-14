use bytes::Bytes;
use serde_json::Value;

pub(crate) const MAX_PROXY_STREAM_EVENT_BYTES: usize = 8 * 1024 * 1024;
pub(crate) const MAX_PROXY_RESPONSES_STREAM_EVENT_BYTES: usize = 128 * 1024 * 1024;
pub(crate) const MAX_PROXY_IMAGE_STREAM_EVENT_BYTES: usize = 72 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProxyStreamProtocol {
    OpenAiResponses,
    OpenAiChat,
    OpenAiImages,
    AnthropicMessages,
    Gemini,
}

impl ProxyStreamProtocol {
    pub(crate) fn from_path(path: &str) -> Option<Self> {
        let path = path
            .split('?')
            .next()
            .unwrap_or(path)
            .trim_end_matches('/')
            .to_ascii_lowercase();
        if matches!(
            path.as_str(),
            "/v1/responses"
                | "/v1/responses/compact"
                | "/v1/v1/responses"
                | "/v1/v1/responses/compact"
                | "/responses"
                | "/responses/compact"
                | "/codex/v1/responses"
                | "/codex/v1/responses/compact"
                | "/openai/v1/responses"
                | "/openai/v1/responses/compact"
                | "/backend-api/codex/responses"
                | "/backend-api/codex/responses/compact"
        ) {
            return Some(Self::OpenAiResponses);
        }
        if matches!(
            path.as_str(),
            "/v1/chat/completions"
                | "/v1/v1/chat/completions"
                | "/chat/completions"
                | "/codex/v1/chat/completions"
                | "/openai/v1/chat/completions"
        ) {
            return Some(Self::OpenAiChat);
        }
        if matches!(
            path.as_str(),
            "/v1/images/generations" | "/images/generations" | "/v1/images/edits" | "/images/edits"
        ) {
            return Some(Self::OpenAiImages);
        }
        if matches!(
            path.as_str(),
            "/v1/messages" | "/claude/v1/messages" | "/anthropic/v1/messages"
        ) {
            return Some(Self::AnthropicMessages);
        }
        let gemini_route = path.starts_with("/v1beta/models/")
            || path.starts_with("/v1/models/")
            || path.starts_with("/gemini/v1/models/")
            || path.starts_with("/gemini/v1beta/models/");
        if gemini_route
            && (path.contains(":streamgeneratecontent")
                || path.contains(":stream_generate_content"))
        {
            return Some(Self::Gemini);
        }
        None
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct ProxyStreamObservation {
    pub(crate) meaningful_progress: bool,
    pub(crate) terminal_chunk_end: Option<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProxyStreamParseError {
    EventTooLarge,
}

#[derive(Debug)]
pub(crate) struct ProxyStreamDetector {
    protocol: ProxyStreamProtocol,
    pending: Vec<u8>,
    scan_from: usize,
    max_event_bytes: usize,
}

impl ProxyStreamDetector {
    pub(crate) fn new(protocol: ProxyStreamProtocol) -> Self {
        let max_event_bytes = match protocol {
            ProxyStreamProtocol::OpenAiResponses => MAX_PROXY_RESPONSES_STREAM_EVENT_BYTES,
            ProxyStreamProtocol::OpenAiImages => MAX_PROXY_IMAGE_STREAM_EVENT_BYTES,
            _ => MAX_PROXY_STREAM_EVENT_BYTES,
        };
        Self::with_limit(protocol, max_event_bytes)
    }

    fn with_limit(protocol: ProxyStreamProtocol, max_event_bytes: usize) -> Self {
        Self {
            protocol,
            pending: Vec::new(),
            scan_from: 0,
            max_event_bytes,
        }
    }

    pub(crate) fn push(
        &mut self,
        chunk: &Bytes,
    ) -> Result<ProxyStreamObservation, ProxyStreamParseError> {
        let previous_pending = self.pending.len();
        self.pending.extend_from_slice(chunk);
        let mut observation = ProxyStreamObservation::default();
        let mut consumed: usize = 0;

        while let Some((event_end, delimiter_len)) =
            next_event_boundary(&self.pending, self.scan_from)
        {
            if event_end > self.max_event_bytes {
                return Err(ProxyStreamParseError::EventTooLarge);
            }
            let boundary_end = event_end + delimiter_len;
            let event = self.pending[..event_end].to_vec();
            let parsed = parse_event(self.protocol, &event);
            observation.meaningful_progress |= parsed.meaningful;
            if parsed.terminal {
                observation.terminal_chunk_end = Some(
                    consumed
                        .saturating_add(boundary_end)
                        .saturating_sub(previous_pending)
                        .min(chunk.len()),
                );
                self.pending.clear();
                self.scan_from = 0;
                return Ok(observation);
            }
            self.pending.drain(..boundary_end);
            self.scan_from = 0;
            consumed = consumed.saturating_add(boundary_end);
        }

        if self.pending.len() > self.max_event_bytes {
            return Err(ProxyStreamParseError::EventTooLarge);
        }
        self.scan_from = self.pending.len().saturating_sub(3);
        Ok(observation)
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct ParsedEvent {
    meaningful: bool,
    terminal: bool,
}

fn parse_event(protocol: ProxyStreamProtocol, event: &[u8]) -> ParsedEvent {
    let Ok(text) = std::str::from_utf8(event) else {
        return ParsedEvent {
            meaningful: event.iter().any(|byte| !byte.is_ascii_whitespace()),
            terminal: false,
        };
    };
    let mut declared_event = None;
    let mut data = Vec::new();
    let mut has_non_comment_field = false;
    for raw_line in text.split('\n') {
        let line = raw_line.strip_suffix('\r').unwrap_or(raw_line);
        if line.is_empty() || line.starts_with(':') {
            continue;
        }
        has_non_comment_field = true;
        if let Some(value) = sse_field(line, "event") {
            declared_event = Some(value.trim());
        } else if let Some(value) = sse_field(line, "data") {
            data.push(value.trim_start());
        }
    }
    let payload = if data.is_empty() {
        let trimmed = text.trim();
        if trimmed.starts_with('{') || trimmed == "[DONE]" {
            Some(trimmed.to_string())
        } else {
            None
        }
    } else {
        Some(data.join("\n").trim().to_string())
    };
    let declared_keepalive = declared_event.is_some_and(is_keepalive_name);
    let payload_keepalive = payload.as_deref().is_some_and(payload_is_keepalive);
    let meaningful = !declared_keepalive
        && !payload_keepalive
        && (payload.as_deref().is_some_and(|value| !value.is_empty())
            || has_non_comment_field && declared_event.is_some());
    let terminal = declared_event.is_some_and(|event| declared_event_is_terminal(protocol, event))
        || payload
            .as_deref()
            .is_some_and(|payload| payload_is_terminal(protocol, payload));
    ParsedEvent {
        meaningful,
        terminal,
    }
}

fn is_keepalive_name(value: &str) -> bool {
    value.eq_ignore_ascii_case("ping") || value.eq_ignore_ascii_case("keepalive")
}

fn payload_is_keepalive(payload: &str) -> bool {
    let payload = payload.trim();
    if is_keepalive_name(payload) {
        return true;
    }
    serde_json::from_str::<Value>(payload)
        .ok()
        .and_then(|value| {
            value
                .get("type")
                .or_else(|| value.get("event"))
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .is_some_and(|value| is_keepalive_name(&value))
}

fn declared_event_is_terminal(protocol: ProxyStreamProtocol, event: &str) -> bool {
    match protocol {
        ProxyStreamProtocol::OpenAiResponses => matches!(
            event,
            "response.completed"
                | "response.failed"
                | "response.incomplete"
                | "response.cancelled"
                | "response.canceled"
                | "error"
        ),
        ProxyStreamProtocol::OpenAiChat => event == "error",
        ProxyStreamProtocol::OpenAiImages => matches!(
            event,
            "image_generation.completed"
                | "image_generation.failed"
                | "image_generation.cancelled"
                | "image_generation.canceled"
                | "image_edit.completed"
                | "image_edit.failed"
                | "image_edit.cancelled"
                | "image_edit.canceled"
                | "error"
        ),
        ProxyStreamProtocol::AnthropicMessages => matches!(event, "message_stop" | "error"),
        ProxyStreamProtocol::Gemini => event == "error",
    }
}

fn payload_is_terminal(protocol: ProxyStreamProtocol, payload: &str) -> bool {
    if payload == "[DONE]" {
        return matches!(
            protocol,
            ProxyStreamProtocol::OpenAiResponses | ProxyStreamProtocol::OpenAiChat
        );
    }
    let Ok(value) = serde_json::from_str::<Value>(payload) else {
        return false;
    };
    if value.get("error").is_some_and(|error| !error.is_null())
        || value.get("type").and_then(Value::as_str) == Some("error")
    {
        return true;
    }
    match protocol {
        ProxyStreamProtocol::OpenAiResponses => {
            let event_type = value.get("type").and_then(Value::as_str);
            let status = value
                .get("response")
                .and_then(|response| response.get("status"))
                .or_else(|| value.get("status"))
                .and_then(Value::as_str);
            matches!(
                event_type,
                Some(
                    "response.completed"
                        | "response.failed"
                        | "response.incomplete"
                        | "response.cancelled"
                        | "response.canceled"
                )
            ) || matches!(
                status,
                Some("completed" | "failed" | "incomplete" | "cancelled" | "canceled")
            )
        }
        ProxyStreamProtocol::OpenAiChat => false,
        ProxyStreamProtocol::OpenAiImages => matches!(
            value.get("type").and_then(Value::as_str),
            Some(
                "image_generation.completed"
                    | "image_generation.failed"
                    | "image_generation.cancelled"
                    | "image_generation.canceled"
                    | "image_edit.completed"
                    | "image_edit.failed"
                    | "image_edit.cancelled"
                    | "image_edit.canceled"
                    | "error"
            )
        ),
        ProxyStreamProtocol::AnthropicMessages => matches!(
            value.get("type").and_then(Value::as_str),
            Some("message_stop" | "error")
        ),
        ProxyStreamProtocol::Gemini => gemini_terminal(&value),
    }
}

fn gemini_terminal(value: &Value) -> bool {
    let value = value.get("response").unwrap_or(value);
    let prompt_blocked = value
        .pointer("/promptFeedback/blockReason")
        .or_else(|| value.pointer("/prompt_feedback/block_reason"))
        .and_then(Value::as_str)
        .is_some_and(|reason| !reason.trim().is_empty());
    let candidate_finished = value
        .get("candidates")
        .and_then(Value::as_array)
        .is_some_and(|candidates| {
            !candidates.is_empty()
                && candidates.iter().all(|candidate| {
                    candidate
                        .get("finishReason")
                        .or_else(|| candidate.get("finish_reason"))
                        .and_then(Value::as_str)
                        .is_some_and(|reason| !reason.trim().is_empty())
                })
        });
    prompt_blocked || candidate_finished
}

fn sse_field<'a>(line: &'a str, field: &str) -> Option<&'a str> {
    let rest = line.strip_prefix(field)?.strip_prefix(':')?;
    Some(rest.strip_prefix(' ').unwrap_or(rest))
}

fn next_event_boundary(buffer: &[u8], scan_from: usize) -> Option<(usize, usize)> {
    for index in scan_from.min(buffer.len())..buffer.len() {
        if buffer.get(index..index + 2) == Some(b"\n\n") {
            return Some((index, 2));
        }
        if buffer.get(index..index + 4) == Some(b"\r\n\r\n") {
            return Some((index, 4));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protocol_paths_cover_all_public_inference_aliases() {
        for path in [
            "/v1/responses",
            "/v1/v1/responses/compact",
            "/responses?stream=true",
            "/codex/v1/responses",
            "/openai/v1/responses/compact",
            "/backend-api/codex/responses",
        ] {
            assert_eq!(
                ProxyStreamProtocol::from_path(path),
                Some(ProxyStreamProtocol::OpenAiResponses),
                "path={path}"
            );
        }
        for path in [
            "/v1/images/generations",
            "/images/generations?stream=true",
            "/v1/images/edits",
            "/images/edits",
        ] {
            assert_eq!(
                ProxyStreamProtocol::from_path(path),
                Some(ProxyStreamProtocol::OpenAiImages),
                "path={path}"
            );
        }
        for path in [
            "/v1/chat/completions",
            "/v1/v1/chat/completions",
            "/chat/completions",
            "/codex/v1/chat/completions",
            "/openai/v1/chat/completions",
        ] {
            assert_eq!(
                ProxyStreamProtocol::from_path(path),
                Some(ProxyStreamProtocol::OpenAiChat),
                "path={path}"
            );
        }
        for path in [
            "/v1/messages",
            "/claude/v1/messages",
            "/anthropic/v1/messages",
        ] {
            assert_eq!(
                ProxyStreamProtocol::from_path(path),
                Some(ProxyStreamProtocol::AnthropicMessages),
                "path={path}"
            );
        }
        for path in [
            "/v1beta/models/gemini:streamGenerateContent",
            "/v1/models/gemini:streamGenerateContent",
            "/gemini/v1/models/gemini:streamGenerateContent",
            "/gemini/v1beta/models/gemini:stream_generate_content?alt=sse",
        ] {
            assert_eq!(
                ProxyStreamProtocol::from_path(path),
                Some(ProxyStreamProtocol::Gemini),
                "path={path}"
            );
        }
        for path in [
            "/v1/messages/count_tokens",
            "/responses/input_tokens",
            "/v1beta/models/gemini:generateContent",
            "/v1/models",
        ] {
            assert_eq!(ProxyStreamProtocol::from_path(path), None, "path={path}");
        }
    }

    #[test]
    fn responses_terminal_is_detected_across_every_split() {
        let event = b"event: response.completed\r\ndata: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\"}}\r\n\r\n";
        for split in 1..event.len() {
            let mut detector = ProxyStreamDetector::new(ProxyStreamProtocol::OpenAiResponses);
            let first = detector
                .push(&Bytes::copy_from_slice(&event[..split]))
                .unwrap();
            assert_eq!(first.terminal_chunk_end, None, "split={split}");
            let second = detector
                .push(&Bytes::copy_from_slice(&event[split..]))
                .unwrap();
            assert_eq!(
                second.terminal_chunk_end,
                Some(event.len() - split),
                "split={split}"
            );
        }
    }

    #[test]
    fn responses_terminal_is_detected_across_single_byte_chunks() {
        let event = b"event: response.completed\r\ndata: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\"}}\r\n\r\n";
        let mut detector = ProxyStreamDetector::new(ProxyStreamProtocol::OpenAiResponses);

        for (index, byte) in event.iter().enumerate() {
            let observation = detector
                .push(&Bytes::copy_from_slice(std::slice::from_ref(byte)))
                .unwrap();
            assert_eq!(
                observation.terminal_chunk_end,
                (index + 1 == event.len()).then_some(1),
                "index={index}"
            );
        }
    }

    #[test]
    fn terminal_offset_excludes_trailing_keepalive() {
        let terminal = b"data: [DONE]\n\n";
        let mut bytes = terminal.to_vec();
        bytes.extend_from_slice(b": irrelevant\n\n");
        let mut detector = ProxyStreamDetector::new(ProxyStreamProtocol::OpenAiChat);
        let observation = detector.push(&Bytes::from(bytes)).unwrap();
        assert_eq!(observation.terminal_chunk_end, Some(terminal.len()));
    }

    #[test]
    fn image_partial_is_progress_but_only_completed_is_terminal() {
        let partial = Bytes::from_static(
            b"event: image_generation.partial_image\ndata: {\"type\":\"image_generation.partial_image\",\"b64_json\":\"iVBORw0KGgo=\"}\n\n",
        );
        let completed = Bytes::from_static(
            b"event: image_generation.completed\ndata: {\"type\":\"image_generation.completed\",\"b64_json\":\"iVBORw0KGgo=\"}\n\n",
        );
        let mut detector = ProxyStreamDetector::new(ProxyStreamProtocol::OpenAiImages);

        let partial_observation = detector.push(&partial).unwrap();
        assert!(partial_observation.meaningful_progress);
        assert_eq!(partial_observation.terminal_chunk_end, None);

        let completed_observation = detector.push(&completed).unwrap();
        assert!(completed_observation.meaningful_progress);
        assert_eq!(
            completed_observation.terminal_chunk_end,
            Some(completed.len())
        );
    }

    #[test]
    fn protocol_event_capacities_match_supported_payloads() {
        assert_eq!(
            ProxyStreamDetector::new(ProxyStreamProtocol::OpenAiResponses).max_event_bytes,
            MAX_PROXY_RESPONSES_STREAM_EVENT_BYTES
        );
        assert_eq!(
            ProxyStreamDetector::new(ProxyStreamProtocol::OpenAiImages).max_event_bytes,
            MAX_PROXY_IMAGE_STREAM_EVENT_BYTES
        );
        for protocol in [
            ProxyStreamProtocol::OpenAiChat,
            ProxyStreamProtocol::AnthropicMessages,
            ProxyStreamProtocol::Gemini,
        ] {
            assert_eq!(
                ProxyStreamDetector::new(protocol).max_event_bytes,
                MAX_PROXY_STREAM_EVENT_BYTES,
                "protocol={protocol:?}"
            );
        }
    }

    #[test]
    fn terminal_offset_includes_prior_event_in_same_chunk() {
        let first = b"data: {\"choices\":[{\"delta\":{\"content\":\"hi\"}}]}\n\n";
        let terminal = b"data: [DONE]\n\n";
        let mut bytes = first.to_vec();
        bytes.extend_from_slice(terminal);
        bytes.extend_from_slice(b": irrelevant\n\n");
        let mut detector = ProxyStreamDetector::new(ProxyStreamProtocol::OpenAiChat);
        let observation = detector.push(&Bytes::from(bytes)).unwrap();
        assert_eq!(
            observation.terminal_chunk_end,
            Some(first.len() + terminal.len())
        );
    }

    #[test]
    fn comments_do_not_count_as_progress() {
        let mut detector = ProxyStreamDetector::new(ProxyStreamProtocol::OpenAiResponses);
        let observation = detector
            .push(&Bytes::from_static(b": keepalive\n\n"))
            .unwrap();
        assert!(!observation.meaningful_progress);
        assert_eq!(observation.terminal_chunk_end, None);
    }

    #[test]
    fn named_and_json_keepalives_do_not_count_as_progress() {
        for event in [
            b"event: ping\ndata: {\"type\":\"ping\"}\n\n".as_slice(),
            b"data: {\"type\":\"keepalive\"}\n\n".as_slice(),
            b"data: ping\n\n".as_slice(),
        ] {
            let mut detector = ProxyStreamDetector::new(ProxyStreamProtocol::OpenAiResponses);
            let observation = detector.push(&Bytes::copy_from_slice(event)).unwrap();
            assert!(!observation.meaningful_progress);
            assert_eq!(observation.terminal_chunk_end, None);
        }
    }

    #[test]
    fn null_error_field_is_not_terminal() {
        let mut detector = ProxyStreamDetector::new(ProxyStreamProtocol::OpenAiResponses);
        let event = Bytes::from_static(b"data: {\"type\":\"response.created\",\"error\":null}\n\n");
        let observation = detector.push(&event).unwrap();
        assert!(observation.meaningful_progress);
        assert_eq!(observation.terminal_chunk_end, None);
    }

    #[test]
    fn protocol_terminals_are_recognized() {
        for (protocol, event) in [
            (
                ProxyStreamProtocol::AnthropicMessages,
                b"event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n".as_slice(),
            ),
            (
                ProxyStreamProtocol::Gemini,
                b"data: {\"candidates\":[{\"finishReason\":\"STOP\"}]}\n\n".as_slice(),
            ),
            (
                ProxyStreamProtocol::Gemini,
                b"data: {\"promptFeedback\":{\"blockReason\":\"SAFETY\"}}\n\n".as_slice(),
            ),
            (
                ProxyStreamProtocol::OpenAiResponses,
                b"data: {\"type\":\"response.failed\"}\n\n".as_slice(),
            ),
        ] {
            let mut detector = ProxyStreamDetector::new(protocol);
            let observation = detector.push(&Bytes::copy_from_slice(event)).unwrap();
            assert_eq!(observation.terminal_chunk_end, Some(event.len()));
        }
    }

    #[test]
    fn event_buffer_is_bounded() {
        let mut detector = ProxyStreamDetector::with_limit(ProxyStreamProtocol::OpenAiChat, 8);
        assert_eq!(
            detector.push(&Bytes::from_static(b"data: 123")),
            Err(ProxyStreamParseError::EventTooLarge)
        );
    }
}
