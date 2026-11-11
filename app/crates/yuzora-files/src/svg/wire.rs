//! What the parent and the child say to each other, little-endian:
//!
//! - request (stdin): `YZSVGIN1`, the longest side to draw (u32), the SVG's
//!   length (u32), the SVG;
//! - answer (stdout): `YZSVGPX1`, a status (u32: [`DRAWN`] or
//!   [`UNREADABLE`]), width and height (u32 each, 0 when not drawn), then when
//!   drawn `width × height × 4` bytes of RGBA, straight alpha.
//!
//! Both sides read exactly the lengths named, never up to an end of file: a
//! pipe end another process holds by accident doesn't keep either waiting.

use std::sync::Arc;

use gpui::RenderImage;

use super::{BAD_OUTPUT, INVALID, MAX_EDGE, MAX_SVG_BYTES};

pub(super) const REQUEST: [u8; 8] = *b"YZSVGIN1";
pub(super) const ANSWER: [u8; 8] = *b"YZSVGPX1";
pub(super) const REQUEST_HEAD: usize = 16;
pub(super) const ANSWER_HEAD: usize = 20;
/// The SVG is drawn: its pixels follow.
pub(super) const DRAWN: u32 = 0;
/// usvg couldn't read or draw it.
pub(super) const UNREADABLE: u32 = 1;

fn u32_at(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

pub(super) fn request_head(max_edge: u32, len: usize) -> [u8; REQUEST_HEAD] {
    let mut head = [0; REQUEST_HEAD];
    head[..8].copy_from_slice(&REQUEST);
    head[8..12].copy_from_slice(&max_edge.to_le_bytes());
    head[12..].copy_from_slice(&(len as u32).to_le_bytes());
    head
}

/// The longest side and the SVG's length a request names, within the limits.
pub(super) fn read_request_head(head: &[u8; REQUEST_HEAD]) -> Option<(u32, usize)> {
    let (max_edge, len) = (u32_at(head, 8), u32_at(head, 12));
    let fits = head[..8] == REQUEST
        && (1..=MAX_EDGE).contains(&max_edge)
        && u64::from(len) <= MAX_SVG_BYTES;
    fits.then_some((max_edge, len as usize))
}

pub(super) fn answer_head(status: u32, width: u32, height: u32) -> [u8; ANSWER_HEAD] {
    let mut head = [0; ANSWER_HEAD];
    head[..8].copy_from_slice(&ANSWER);
    head[8..12].copy_from_slice(&status.to_le_bytes());
    head[12..16].copy_from_slice(&width.to_le_bytes());
    head[16..].copy_from_slice(&height.to_le_bytes());
    head
}

/// An SVG's pixels, RGBA.
#[derive(Debug)]
pub(super) struct Raster {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl Raster {
    /// The image GPUI shows: BGRA, at the scale its own SVG drawing has.
    pub fn into_image(self) -> Arc<RenderImage> {
        let Raster {
            width,
            height,
            mut rgba,
        } = self;
        for px in rgba.chunks_exact_mut(4) {
            px.swap(0, 2);
        }
        let frame = ::image::RgbaImage::from_raw(width, height, rgba)
            .map(::image::Frame::new)
            .into_iter()
            .collect::<Vec<_>>();
        Arc::new(RenderImage::new(frame).with_scale_factor(gpui::SMOOTH_SVG_SCALE_FACTOR))
    }
}

/// What the parent has read of an answer. It holds no more than the header
/// and the pixels the header names, each side at most the request's.
pub(super) struct Answer {
    max_edge: u32,
    head: [u8; ANSWER_HEAD],
    filled: usize,
    status: u32,
    width: u32,
    height: u32,
    /// Once the header is read: the pixels, and how many are due.
    pixels: Option<Vec<u8>>,
    due: usize,
}

impl Answer {
    pub fn new(max_edge: u32) -> Self {
        Self {
            max_edge,
            head: [0; ANSWER_HEAD],
            filled: 0,
            status: 0,
            width: 0,
            height: 0,
            pixels: None,
            due: 0,
        }
    }

    /// Takes bytes read from the child. Err: not an answer (a header that
    /// isn't one or names too much, or bytes past what it names).
    pub fn take(&mut self, mut data: &[u8]) -> Result<(), &'static str> {
        while !data.is_empty() {
            if self.filled < ANSWER_HEAD {
                let n = data.len().min(ANSWER_HEAD - self.filled);
                self.head[self.filled..self.filled + n].copy_from_slice(&data[..n]);
                self.filled += n;
                data = &data[n..];
                if self.filled == ANSWER_HEAD {
                    self.open()?;
                }
                continue;
            }
            let (Some(pixels), due) = (self.pixels.as_mut(), self.due) else {
                return Err(BAD_OUTPUT);
            };
            let n = data.len().min(due - pixels.len());
            if n == 0 {
                return Err(BAD_OUTPUT);
            }
            pixels.extend_from_slice(&data[..n]);
            data = &data[n..];
        }
        Ok(())
    }

    fn open(&mut self) -> Result<(), &'static str> {
        let head = &self.head;
        let (status, width, height) = (u32_at(head, 8), u32_at(head, 12), u32_at(head, 16));
        let sized = |side: u32| (1..=self.max_edge).contains(&side);
        let due = match status {
            _ if head[..8] != ANSWER => return Err(BAD_OUTPUT),
            DRAWN if sized(width) && sized(height) => width as usize * height as usize * 4,
            UNREADABLE if width == 0 && height == 0 => 0,
            _ => return Err(BAD_OUTPUT),
        };
        (self.status, self.width, self.height, self.due) = (status, width, height, due);
        self.pixels = Some(Vec::with_capacity(due));
        Ok(())
    }

    /// The pixels, once all are read.
    pub fn finish(self) -> Result<Raster, &'static str> {
        match self.pixels {
            Some(rgba) if rgba.len() == self.due => match self.status {
                DRAWN => Ok(Raster {
                    width: self.width,
                    height: self.height,
                    rgba,
                }),
                _ => Err(INVALID),
            },
            _ => Err(BAD_OUTPUT),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn answer(max_edge: u32, bytes: &[u8]) -> Result<Raster, &'static str> {
        let mut a = Answer::new(max_edge);
        a.take(bytes)?;
        a.finish()
    }

    #[test]
    fn an_answer_is_read_in_any_pieces() {
        let mut bytes = answer_head(DRAWN, 2, 1).to_vec();
        bytes.extend([1, 2, 3, 4, 5, 6, 7, 8]);
        let mut a = Answer::new(4);
        for b in &bytes {
            a.take(std::slice::from_ref(b)).unwrap();
        }
        let r = a.finish().unwrap();
        assert_eq!(
            (r.width, r.height, r.rgba),
            (2, 1, vec![1, 2, 3, 4, 5, 6, 7, 8])
        );
        // GPUI's order.
        let image = answer(4, &bytes).unwrap().into_image();
        assert_eq!(image.as_bytes(0).unwrap(), [3, 2, 1, 4, 7, 6, 5, 8]);
    }

    #[test]
    fn answers_that_say_too_much_or_too_little_are_refused() {
        let mut full = answer_head(DRAWN, 1, 1).to_vec();
        full.extend([0; 4]);
        // One byte past what it names.
        let mut more = full.clone();
        more.push(0);
        assert_eq!(answer(4, &more).unwrap_err(), BAD_OUTPUT);
        // Cut short.
        assert_eq!(answer(4, &full[..full.len() - 1]).unwrap_err(), BAD_OUTPUT);
        assert_eq!(answer(4, &full[..10]).unwrap_err(), BAD_OUTPUT);
        // Larger than asked for: refused before anything is held.
        let mut a = Answer::new(4);
        assert_eq!(a.take(&answer_head(DRAWN, 5, 1)).unwrap_err(), BAD_OUTPUT);
        assert!(a.pixels.is_none());
        assert_eq!(
            answer(4, &answer_head(DRAWN, 0, 1)).unwrap_err(),
            BAD_OUTPUT
        );
        // Another magic, another status.
        let mut other = full.clone();
        other[0] = b'X';
        assert_eq!(answer(4, &other).unwrap_err(), BAD_OUTPUT);
        assert_eq!(answer(4, &answer_head(7, 0, 0)).unwrap_err(), BAD_OUTPUT);
        // Not drawn: just the header.
        assert_eq!(
            answer(4, &answer_head(UNREADABLE, 0, 0)).unwrap_err(),
            INVALID
        );
        let mut unreadable = answer_head(UNREADABLE, 0, 0).to_vec();
        unreadable.push(0);
        assert_eq!(answer(4, &unreadable).unwrap_err(), BAD_OUTPUT);
    }

    #[test]
    fn requests_name_their_limits() {
        let head = request_head(64, 10);
        assert_eq!(read_request_head(&head), Some((64, 10)));
        assert_eq!(read_request_head(&request_head(MAX_EDGE + 1, 10)), None);
        assert_eq!(read_request_head(&request_head(0, 10)), None);
        assert_eq!(
            read_request_head(&request_head(64, MAX_SVG_BYTES as usize + 1)),
            None
        );
        let mut other = head;
        other[0] = b'X';
        assert_eq!(read_request_head(&other), None);
    }
}
