//! LSP `Content-Length` framing：JSON-RPC over stdio 的增量解帧与编码。
//!
//! 协议形态：每个消息由一段 header 与一个 body 组成
//!
//! ```text
//! Content-Length: <N>\r\n
//! [\r\n]*               ; 可选的其他 header，每个以 \r\n 结尾
//! \r\n                  ; 结束 header 段的空行
//! <N 字节 body>          ; JSON-RPC 消息
//! ```
//!
//! 本模块不复用 SSE / JSONL / partial-json 解析器，按协议自实现严格状态机：
//!
//! - header / body 可跨越任意 chunk 边界（增量 `feed` + 反复 `next`）。
//! - 多个连续 frame 在一次 feed 后可被逐个取出。
//! - 大小上限在解析 `Content-Length` 值时即生效，**先校验后分配**。
//! - 重复 / 缺失 / 非法 header、非 token header name、EOF 半帧均给出有界错误。

use crate::error::FrameError;

/// 解析器硬性绝对上限（即使调用方配置更大也以此兜底，防恶意巨型 Content-Length）。
pub const MAX_FRAME_BYTES_HARD_LIMIT: u64 = 64 * 1024 * 1024; // 64 MiB

/// header 段的严格上界：超过即判定为恶意 / 损坏流，防止在找到 `\r\n\r\n` 前
/// 无限缓冲 header 字节。
pub const MAX_HEADER_BYTES: usize = 64 * 1024; // 64 KiB

/// 单次 `next` 的产出。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FrameEvent {
    /// 完整解出一帧（body 字节）。
    Complete(Vec<u8>),
    /// 缓冲区暂时不足以解出下一帧；调用方应继续 `feed` 后再 `next`。
    NeedMoreData,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DecodeState {
    /// 正在读取 header 段；`scan_from` 是下一次查找 `\r\n\r\n` 的起点。
    ReadingHeaders { scan_from: usize },
    /// 已读出 header，正在读取 body；`body_start` 是 body 在 buf 中的起点。
    ReadingBody { body_start: usize, body_len: usize },
    /// 解析遇到致命错误，解析器已停用。
    Poisoned,
}

/// 增量 `Content-Length` 帧解码器。
pub struct LspFrameDecoder {
    buf: Vec<u8>,
    state: DecodeState,
    max_frame_bytes: u64,
}

impl std::fmt::Debug for LspFrameDecoder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LspFrameDecoder")
            .field("state", &self.state)
            .field("max_frame_bytes", &self.max_frame_bytes)
            .field("buffered", &self.buf.len())
            .finish()
    }
}

impl LspFrameDecoder {
    pub fn new(max_frame_bytes: u64) -> Self {
        let max = max_frame_bytes.clamp(1, MAX_FRAME_BYTES_HARD_LIMIT);
        Self {
            buf: Vec::new(),
            state: DecodeState::ReadingHeaders { scan_from: 0 },
            max_frame_bytes: max,
        }
    }

    /// 当前配置的单帧 body 字节上限。
    pub fn max_frame_bytes(&self) -> u64 {
        self.max_frame_bytes
    }

    /// 追加一段字节流（chunk 可任意切分）。
    pub fn feed(&mut self, bytes: &[u8]) {
        if matches!(self.state, DecodeState::Poisoned) {
            return;
        }
        self.buf.extend_from_slice(bytes);
    }

    /// 流已结束时调用：若仍有未完成 frame，返回对应的半帧错误。
    pub fn finish(&mut self) -> Result<(), FrameError> {
        match self.state {
            DecodeState::Poisoned => Ok(()),
            DecodeState::ReadingHeaders { .. } if self.buf.is_empty() => Ok(()),
            DecodeState::ReadingHeaders { .. } => Err(FrameError::UnexpectedEof(self.buf.len())),
            DecodeState::ReadingBody {
                body_start,
                body_len,
            } => {
                let _ = (body_start, body_len);
                Err(FrameError::UnexpectedEof(self.buf.len()))
            }
        }
    }

    /// 尝试解出下一帧。重复调用直到返回 [`FrameEvent::NeedMoreData`]。
    ///
    /// 返回 [`FrameError`] 后解码器进入 poisoned 状态，必须重连后换新解码器。
    pub fn decode_next(&mut self) -> Result<FrameEvent, FrameError> {
        let result = match self.state {
            DecodeState::Poisoned => return Err(FrameError::UnexpectedEof(0)),
            DecodeState::ReadingHeaders { scan_from } => self.step_headers(scan_from),
            DecodeState::ReadingBody {
                body_start,
                body_len,
            } => self.step_body(body_start, body_len),
        };
        if result.is_err() {
            // 致命 framing 错误：流已损坏，置为 Poisoned，后续 next/finish 不再解帧。
            self.state = DecodeState::Poisoned;
        }
        result
    }

    fn step_headers(&mut self, scan_from: usize) -> Result<FrameEvent, FrameError> {
        let start = scan_from.min(self.buf.len());
        match find_subsequence(&self.buf[start..], b"\r\n\r\n") {
            None => {
                // ReadingHeaders 状态下 buf[0..] 就是 header 段：未找到终止符且
                // 已超上界 → 拒绝，避免恶意对端无限喂 header。
                if self.buf.len() > MAX_HEADER_BYTES {
                    return Err(FrameError::HeaderTooLarge {
                        max: MAX_HEADER_BYTES,
                    });
                }
                // 重设 scan_from 以覆盖跨 chunk 边界（回看 3 字节）。
                self.state = DecodeState::ReadingHeaders {
                    scan_from: self.buf.len().saturating_sub(3),
                };
                Ok(FrameEvent::NeedMoreData)
            }
            Some(rel) => {
                let header_end = start + rel + 4;
                // buf[0..header_end] 即完整 header 段（ReadingHeaders 状态下 buf 从
                // 帧头开始）：即使单次 feed 已含终止符，也按上界拒绝超大 header。
                if header_end > MAX_HEADER_BYTES {
                    return Err(FrameError::HeaderTooLarge {
                        max: MAX_HEADER_BYTES,
                    });
                }
                let content_length = parse_headers(&self.buf[..header_end], self.max_frame_bytes)?;
                let body_end = header_end + content_length;
                if body_end > self.buf.len() {
                    self.state = DecodeState::ReadingBody {
                        body_start: header_end,
                        body_len: content_length,
                    };
                    Ok(FrameEvent::NeedMoreData)
                } else {
                    let frame = self.extract(header_end, body_end);
                    self.state = DecodeState::ReadingHeaders { scan_from: 0 };
                    Ok(FrameEvent::Complete(frame))
                }
            }
        }
    }

    fn step_body(&mut self, body_start: usize, body_len: usize) -> Result<FrameEvent, FrameError> {
        let body_end = body_start + body_len;
        if body_end > self.buf.len() {
            self.state = DecodeState::ReadingBody {
                body_start,
                body_len,
            };
            Ok(FrameEvent::NeedMoreData)
        } else {
            let frame = self.extract(body_start, body_end);
            self.state = DecodeState::ReadingHeaders { scan_from: 0 };
            Ok(FrameEvent::Complete(frame))
        }
    }

    /// 从 buf 中切出 [body_start, body_end) 作为 frame，保留 [body_end, ...) 为剩余。
    fn extract(&mut self, body_start: usize, body_end: usize) -> Vec<u8> {
        // split_off(body_end) 把 buf 留为 [0, body_end)，remainder 为 [body_end, len)。
        let remainder = self.buf.split_off(body_end);
        // 只拷贝 body（返回给调用方，本就必要），不再 drain 移位整个剩余缓冲——
        // 旧实现对「单 chunk 多帧」是 O(n²)，此处每个字节只被处理常数次。
        let frame = self.buf[body_start..body_end].to_vec();
        self.buf = remainder;
        frame
    }
}

/// 解析 header 段（以 `\r\n\r\n` 结尾），返回 Content-Length（字节）。
fn parse_headers(header_bytes: &[u8], max_frame_bytes: u64) -> Result<usize, FrameError> {
    // header 段以 `\r\n\r\n` 结尾：末两字节是「空行」终止符；其余是若干
    // `Name: Value\r\n`。这里去掉末尾空行（2 字节），保留每个字段的 `\r\n`。
    let body = &header_bytes[..header_bytes.len().saturating_sub(2)];
    let mut content_length: Option<u64> = None;
    let mut idx = 0usize;
    while idx < body.len() {
        let line_end = find_subsequence(&body[idx..], b"\r\n")
            .map(|rel| idx + rel)
            .ok_or_else(|| FrameError::MalformedHeader("missing CRLF".into()))?;
        let line = &body[idx..line_end];
        idx = line_end + 2;
        if line.is_empty() {
            continue;
        }
        let colon = find_subsequence(line, b":")
            .ok_or_else(|| FrameError::MalformedHeader("header line missing ':'".into()))?;
        let name = &line[..colon];
        validate_header_name(name)?;
        let value = trim_ows(&line[colon + 1..]);
        if name.eq_ignore_ascii_case(b"Content-Length") {
            if content_length.is_some() {
                return Err(FrameError::DuplicateContentLength);
            }
            let s = std::str::from_utf8(value).map_err(|_| {
                FrameError::InvalidContentLength(String::from_utf8_lossy(value).into_owned())
            })?;
            let n: u64 = s
                .parse()
                .map_err(|_| FrameError::InvalidContentLength(s.to_string()))?;
            content_length = Some(n);
        }
        // 其他 header（含 Content-Type）允许但忽略。
    }
    let n = content_length.ok_or(FrameError::MissingContentLength)?;
    if n > max_frame_bytes {
        return Err(FrameError::OversizedFrame {
            declared: n,
            max: max_frame_bytes,
        });
    }
    Ok(usize::try_from(n).expect("checked <= max_frame_bytes which fits usize"))
}

fn validate_header_name(name: &[u8]) -> Result<(), FrameError> {
    if name.is_empty() {
        return Err(FrameError::MalformedHeader("empty header name".into()));
    }
    for &b in name {
        if !(b.is_ascii_alphanumeric() || b == b'-') {
            return Err(FrameError::MalformedHeader(format!(
                "non-token byte 0x{b:02x} in header name"
            )));
        }
    }
    Ok(())
}

fn trim_ows(bytes: &[u8]) -> &[u8] {
    let mut start = 0;
    while start < bytes.len() && (bytes[start] == b' ' || bytes[start] == b'\t') {
        start += 1;
    }
    let mut end = bytes.len();
    while end > start && (bytes[end - 1] == b' ' || bytes[end - 1] == b'\t') {
        end -= 1;
    }
    &bytes[start..end]
}

fn find_subsequence(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() {
        return Some(0);
    }
    haystack.windows(needle.len()).position(|w| w == needle)
}

/// 把一条 JSON-RPC body 编码为 `Content-Length` 帧（`header + body`）。
pub fn encode_message(body: &[u8]) -> Vec<u8> {
    let header = format!("Content-Length: {}\r\n\r\n", body.len());
    let mut out = Vec::with_capacity(header.len() + body.len());
    out.extend_from_slice(header.as_bytes());
    out.extend_from_slice(body);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decode_all(decoder: &mut LspFrameDecoder) -> Result<Vec<Vec<u8>>, FrameError> {
        let mut out = Vec::new();
        while let FrameEvent::Complete(b) = decoder.decode_next()? {
            out.push(b);
        }
        Ok(out)
    }

    #[test]
    fn encodes_message_with_content_length() {
        let body = br#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#;
        let frame = encode_message(body);
        let expected = format!(
            "Content-Length: {}\r\n\r\n{}",
            body.len(),
            std::str::from_utf8(body).unwrap()
        );
        assert_eq!(frame, expected.as_bytes());
    }

    #[test]
    fn decodes_single_frame_in_one_chunk() {
        let mut d = LspFrameDecoder::new(1024);
        d.feed(&encode_message(b"{\"id\":1}"));
        assert_eq!(decode_all(&mut d).unwrap(), vec![b"{\"id\":1}".to_vec()]);
        assert!(matches!(d.finish(), Ok(())));
    }

    #[test]
    fn decodes_frame_split_across_arbitrary_chunk_boundaries() {
        let frame = encode_message(b"{\"jsonrpc\":\"2.0\",\"id\":7,\"method\":\"x\"}");
        let body = b"{\"jsonrpc\":\"2.0\",\"id\":7,\"method\":\"x\"}".to_vec();
        // 对每个严格小于全长的切分：先喂前半（不应出帧），再喂后半（应正好出一帧）。
        for split in 0..frame.len() {
            let mut d = LspFrameDecoder::new(1024);
            d.feed(&frame[..split]);
            assert!(
                decode_all(&mut d).unwrap().is_empty(),
                "premature frame at {split}"
            );
            d.feed(&frame[split..]);
            assert_eq!(
                decode_all(&mut d).unwrap(),
                vec![body.clone()],
                "failed at split={split}"
            );
            assert!(
                matches!(d.finish(), Ok(())),
                "finish not ok at split={split}"
            );
        }
        // 全长一次喂入：出一帧。
        let mut d = LspFrameDecoder::new(1024);
        d.feed(&frame);
        assert_eq!(decode_all(&mut d).unwrap(), vec![body]);
    }

    #[test]
    fn decodes_frame_byte_by_byte() {
        let frame = encode_message(b"{\"id\":42}");
        let mut d = LspFrameDecoder::new(1024);
        for b in frame.iter() {
            d.feed(std::slice::from_ref(b));
        }
        assert_eq!(decode_all(&mut d).unwrap(), vec![b"{\"id\":42}".to_vec()]);
    }

    #[test]
    fn decodes_two_back_to_back_frames_in_one_chunk() {
        let mut raw = encode_message(b"{\"id\":1}");
        raw.extend(encode_message(b"{\"id\":2}"));
        let mut d = LspFrameDecoder::new(1024);
        d.feed(&raw);
        assert_eq!(
            decode_all(&mut d).unwrap(),
            vec![b"{\"id\":1}".to_vec(), b"{\"id\":2}".to_vec()]
        );
    }

    #[test]
    fn decodes_back_to_back_split_across_boundary() {
        let f1 = encode_message(b"{\"id\":1}");
        let f2 = encode_message(b"{\"id\":2}");
        let mut combined = f1.clone();
        combined.extend(&f2);
        for split in 0..=combined.len() {
            let mut d = LspFrameDecoder::new(1024);
            d.feed(&combined[..split]);
            let mut got = decode_all(&mut d).unwrap();
            d.feed(&combined[split..]);
            got.extend(decode_all(&mut d).unwrap());
            assert_eq!(
                got,
                vec![b"{\"id\":1}".to_vec(), b"{\"id\":2}".to_vec()],
                "split={split}"
            );
        }
    }

    #[test]
    fn rejects_missing_content_length() {
        let mut d = LspFrameDecoder::new(1024);
        d.feed(b"Content-Type: application/vscode-jsonrpc\r\n\r\n{\"id\":1}");
        assert!(matches!(
            d.decode_next().unwrap_err(),
            FrameError::MissingContentLength
        ));
    }

    #[test]
    fn rejects_duplicate_content_length() {
        let mut d = LspFrameDecoder::new(1024);
        d.feed(b"Content-Length: 1\r\nContent-Length: 2\r\n\r\n{}");
        assert!(matches!(
            d.decode_next().unwrap_err(),
            FrameError::DuplicateContentLength
        ));
    }

    #[test]
    fn rejects_invalid_content_length_value() {
        let mut d = LspFrameDecoder::new(1024);
        d.feed(b"Content-Length: not-a-number\r\n\r\n{}");
        assert!(matches!(
            d.decode_next().unwrap_err(),
            FrameError::InvalidContentLength(_)
        ));
    }

    #[test]
    fn rejects_oversized_frame_before_allocation() {
        let mut d = LspFrameDecoder::new(8);
        d.feed(b"Content-Length: 999999\r\n\r\n{}");
        assert!(matches!(
            d.decode_next().unwrap_err(),
            FrameError::OversizedFrame { .. }
        ));
    }

    #[test]
    fn rejects_header_exceeding_cap_without_terminator() {
        let mut d = LspFrameDecoder::new(1024);
        d.feed(&vec![b'a'; MAX_HEADER_BYTES + 1]);
        assert!(matches!(
            d.decode_next().unwrap_err(),
            FrameError::HeaderTooLarge { .. }
        ));
    }

    #[test]
    fn rejects_header_exceeding_cap_even_with_terminator_in_one_chunk() {
        // 单次 feed 已含 `\r\n\r\n`，但 header 段本身超上界 → 仍拒绝。
        let mut raw = vec![b'x'; MAX_HEADER_BYTES + 1];
        raw.extend(b"\r\n\r\n{}");
        let mut d = LspFrameDecoder::new(1024);
        d.feed(&raw);
        assert!(matches!(
            d.decode_next().unwrap_err(),
            FrameError::HeaderTooLarge { .. }
        ));
    }

    #[test]
    fn accepts_header_within_cap() {
        // 长 header 值（含大量 padding 的 Content-Type）在上界内正常解析。
        let body = b"{\"id\":1}";
        let mut raw = format!("Content-Length: {}\r\n", body.len()).into_bytes();
        raw.extend(b"Content-Type: application/x-custom; pad=");
        raw.extend(vec![b'p'; MAX_HEADER_BYTES / 2]);
        raw.extend(b"\r\n\r\n");
        raw.extend(body);
        let mut d = LspFrameDecoder::new(1024);
        d.feed(&raw);
        assert_eq!(decode_all(&mut d).unwrap(), vec![body.to_vec()]);
    }

    #[test]
    fn decodes_many_frames_from_one_chunk() {
        // 单 chunk 内 2000 个连续帧：验证 extract 不再按剩余缓冲整体移位。
        let mut raw = Vec::new();
        for i in 0..2000u32 {
            raw.extend(encode_message(format!("{{\"id\":{i}}}").as_bytes()));
        }
        let mut d = LspFrameDecoder::new(64 * 1024);
        d.feed(&raw);
        let mut count = 0;
        while let FrameEvent::Complete(_) = d.decode_next().unwrap() {
            count += 1;
        }
        assert_eq!(count, 2000);
    }

    #[test]
    fn rejects_malformed_header_line_without_colon() {
        let mut d = LspFrameDecoder::new(1024);
        d.feed(b"NoColonHere\r\n\r\n{}");
        assert!(matches!(
            d.decode_next().unwrap_err(),
            FrameError::MalformedHeader(_)
        ));
    }

    #[test]
    fn rejects_non_token_header_name() {
        let mut d = LspFrameDecoder::new(1024);
        d.feed(b"Bad Name: 1\r\n\r\n{}");
        assert!(matches!(
            d.decode_next().unwrap_err(),
            FrameError::MalformedHeader(_)
        ));
    }

    #[test]
    fn rejects_eof_with_partial_header() {
        let mut d = LspFrameDecoder::new(1024);
        d.feed(b"Content-Length: 100\r\n");
        assert!(decode_all(&mut d).unwrap().is_empty());
        assert!(matches!(
            d.finish().unwrap_err(),
            FrameError::UnexpectedEof(_)
        ));
    }

    #[test]
    fn rejects_eof_with_partial_body() {
        let mut d = LspFrameDecoder::new(1024);
        d.feed(b"Content-Length: 100\r\n\r\n{partial");
        assert!(decode_all(&mut d).unwrap().is_empty());
        assert!(matches!(
            d.finish().unwrap_err(),
            FrameError::UnexpectedEof(_)
        ));
    }

    #[test]
    fn accepts_optional_content_type_header() {
        let body = b"{\"id\":1}";
        let mut raw = format!("Content-Length: {}\r\n", body.len()).into_bytes();
        raw.extend(b"Content-Type: application/vscode-jsonrpc; charset=utf-8\r\n\r\n");
        raw.extend(body);
        let mut d = LspFrameDecoder::new(1024);
        d.feed(&raw);
        assert_eq!(decode_all(&mut d).unwrap(), vec![body.to_vec()]);
    }

    #[test]
    fn decoder_is_poisoned_after_fatal_error() {
        let mut d = LspFrameDecoder::new(1024);
        d.feed(b"Content-Length: nope\r\n\r\n{}");
        let _ = d.decode_next();
        assert!(matches!(
            d.decode_next().unwrap_err(),
            FrameError::UnexpectedEof(_)
        ));
        d.feed(&encode_message(b"{\"id\":1}"));
        assert!(matches!(
            d.decode_next().unwrap_err(),
            FrameError::UnexpectedEof(_)
        ));
    }

    #[test]
    fn cap_is_clamped_to_hard_limit() {
        let d = LspFrameDecoder::new(u64::MAX);
        assert_eq!(d.max_frame_bytes(), MAX_FRAME_BYTES_HARD_LIMIT);
    }
}
