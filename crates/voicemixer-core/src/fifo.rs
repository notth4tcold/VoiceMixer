//! Fixed-capacity frame FIFO between audio API callbacks and the engine.
//! Always stores [`MAX_CHANNELS`] interleaved channels; overflow drops the
//! oldest frames and underflow is filled with silence.

use crate::params::MAX_CHANNELS;

pub struct Fifo {
    buf: Vec<f32>,
    cap: usize,
    head: usize,
    len: usize,
}

impl Fifo {
    pub fn new(cap_frames: usize) -> Self {
        Self {
            buf: vec![0.0; cap_frames * MAX_CHANNELS],
            cap: cap_frames,
            head: 0,
            len: 0,
        }
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn clear(&mut self) {
        self.len = 0;
    }

    pub fn discard(&mut self, frames: usize) {
        let k = frames.min(self.len);
        self.head = (self.head + k) % self.cap;
        self.len -= k;
    }

    fn make_room(&mut self, frames: usize) {
        if self.len + frames > self.cap {
            self.discard(self.len + frames - self.cap);
        }
    }

    #[inline]
    fn slot(&mut self, frame: usize) -> &mut [f32] {
        let i = ((self.head + frame) % self.cap) * MAX_CHANNELS;
        &mut self.buf[i..i + MAX_CHANNELS]
    }

    /// Appends interleaved frames. Mono input is copied to both front channels.
    pub fn push_interleaved(&mut self, src: &[f32], src_ch: usize) {
        if src_ch == 0 {
            return;
        }
        let frames = (src.len() / src_ch).min(self.cap);
        let src = &src[src.len() - frames * src_ch..];
        self.make_room(frames);
        let start = self.len;
        for f in 0..frames {
            let frame = &src[f * src_ch..(f + 1) * src_ch];
            let dst = self.slot(start + f);
            dst.fill(0.0);
            if src_ch == 1 {
                dst[0] = frame[0];
                dst[1] = frame[0];
            } else {
                let k = src_ch.min(MAX_CHANNELS);
                dst[..k].copy_from_slice(&frame[..k]);
            }
        }
        self.len += frames;
    }

    pub fn push_planar(&mut self, chans: &[Vec<f32>], n: usize) {
        let n = n.min(self.cap);
        self.make_room(n);
        let start = self.len;
        for f in 0..n {
            let dst = self.slot(start + f);
            for (c, v) in dst.iter_mut().enumerate() {
                *v = chans.get(c).map_or(0.0, |ch| ch[f]);
            }
        }
        self.len += n;
    }

    /// Pops `n` frames into planar `dst[..channels]`, zero-filling any shortfall.
    pub fn pop_planar(&mut self, dst: &mut [Vec<f32>], channels: usize, n: usize) -> usize {
        let take = n.min(self.len);
        for f in 0..take {
            let i = ((self.head + f) % self.cap) * MAX_CHANNELS;
            for c in 0..channels {
                dst[c][f] = self.buf[i + c];
            }
        }
        for ch in dst.iter_mut().take(channels) {
            ch[take..n].fill(0.0);
        }
        self.discard(take);
        take
    }

    /// Pops interleaved frames into `dst` with `dst_ch` channels. A mono
    /// destination receives the average of the front pair.
    pub fn pop_interleaved(&mut self, dst: &mut [f32], dst_ch: usize) -> usize {
        if dst_ch == 0 {
            return 0;
        }
        let n = dst.len() / dst_ch;
        let take = n.min(self.len);
        for f in 0..take {
            let i = ((self.head + f) % self.cap) * MAX_CHANNELS;
            let out = &mut dst[f * dst_ch..(f + 1) * dst_ch];
            if dst_ch == 1 {
                out[0] = 0.5 * (self.buf[i] + self.buf[i + 1]);
            } else {
                for (c, v) in out.iter_mut().enumerate() {
                    *v = if c < MAX_CHANNELS {
                        self.buf[i + c]
                    } else {
                        0.0
                    };
                }
            }
        }
        dst[take * dst_ch..].fill(0.0);
        self.discard(take);
        take
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_and_overflow() {
        let mut f = Fifo::new(4);
        f.push_interleaved(&[1.0, 2.0, 3.0, 4.0], 2);
        assert_eq!(f.len(), 2);
        let mut out = [0.0; 4];
        f.pop_interleaved(&mut out, 2);
        assert_eq!(out, [1.0, 2.0, 3.0, 4.0]);
        f.push_interleaved(&[1.0, 2.0, 3.0, 4.0, 5.0, 6.0], 1);
        assert_eq!(f.len(), 4);
        let mut out = [0.0; 4];
        f.pop_interleaved(&mut out, 1);
        assert_eq!(out, [3.0, 4.0, 5.0, 6.0]);
    }

    #[test]
    fn underflow_is_silence() {
        let mut f = Fifo::new(8);
        f.push_interleaved(&[0.5, 0.5], 2);
        let mut dst = vec![vec![9.0; 4], vec![9.0; 4]];
        assert_eq!(f.pop_planar(&mut dst, 2, 4), 1);
        assert_eq!(dst[0], vec![0.5, 0.0, 0.0, 0.0]);
    }
}
