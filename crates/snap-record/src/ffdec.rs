//! Video-only decoder backed by an ffmpeg child process emitting raw BGRA frames.

use std::{
    io::Read,
    path::{Path, PathBuf},
    process::{Child, ChildStdout, Stdio},
};

use snap_core::{Error, Result};

use crate::{
    clock::{HNS_PER_SEC, Hns},
    decode::{Decoded, Decoder},
    edit::VideoInfo,
    ffmpeg,
    frame::Frame,
};

pub(crate) struct FfmpegDecoder {
    ffmpeg: PathBuf,
    path: PathBuf,
    info: VideoInfo,
    size: (u32, u32),
    fps: f64,
    start: Hns,
    index: u64,
    child: Option<(Child, ChildStdout)>,
}

impl FfmpegDecoder {
    /// Frames are delivered at `size` and a constant `fps`, converted by ffmpeg.
    pub fn open(ffmpeg: &Path, path: &Path, info: VideoInfo, size: (u32, u32), fps: f64) -> Self {
        Self { ffmpeg: ffmpeg.to_path_buf(), path: path.to_path_buf(), info, size, fps, start: 0, index: 0, child: None }
    }

    fn spawn(&mut self, from_ms: u64) -> Result<()> {
        self.kill();
        let mut cmd = ffmpeg::command(&self.ffmpeg);
        cmd.arg("-ss")
            .arg(format!("{:.3}", from_ms as f64 / 1000.0))
            .arg("-i")
            .arg(&self.path)
            .arg("-an")
            .arg("-vf")
            .arg(format!("fps={:.4},scale={}:{}:flags=lanczos", self.fps, self.size.0, self.size.1))
            .args(["-f", "rawvideo", "-pix_fmt", "bgra", "-"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        let mut child = cmd.spawn()?;
        let out = child.stdout.take().expect("stdout piped");
        self.child = Some((child, out));
        self.start = from_ms as Hns * (HNS_PER_SEC / 1000);
        self.index = 0;
        Ok(())
    }

    fn kill(&mut self) {
        if let Some((mut c, out)) = self.child.take() {
            drop(out);
            let _ = c.kill();
            let _ = c.wait();
        }
    }
}

impl Drop for FfmpegDecoder {
    fn drop(&mut self) {
        self.kill();
    }
}

impl Decoder for FfmpegDecoder {
    fn info(&self) -> VideoInfo {
        self.info.clone()
    }

    fn seek(&mut self, ms: u64) -> Result<()> {
        self.spawn(ms)
    }

    fn next(&mut self) -> Result<Option<Decoded>> {
        if self.child.is_none() {
            self.spawn(0)?;
        }
        let (_, out) = self.child.as_mut().expect("spawned above");
        let mut data = vec![0u8; self.size.0 as usize * self.size.1 as usize * 4];
        match out.read_exact(&mut data) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
            Err(e) => return Err(Error::Io(e)),
        }
        data.iter_mut().skip(3).step_by(4).for_each(|a| *a = 255);
        let pts = self.start + (self.index as f64 / self.fps * HNS_PER_SEC as f64) as Hns;
        self.index += 1;
        Ok(Some(Decoded::Video(Frame { w: self.size.0, h: self.size.1, data }, pts)))
    }
}
