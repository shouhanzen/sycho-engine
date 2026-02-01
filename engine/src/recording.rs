use std::{
    ffi::OsString,
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
    process::{Child, ChildStdin, Command, Stdio},
};

fn ffmpeg_bin() -> OsString {
    std::env::var_os("ROLLOUT_FFMPEG_BIN").unwrap_or_else(|| OsString::from("ffmpeg"))
}

#[derive(Debug, Clone, Copy)]
pub struct Mp4Config {
    pub width: u32,
    pub height: u32,
    pub fps: u32,
}

impl Mp4Config {
    pub fn rgba_frame_len(&self) -> usize {
        (self.width as usize)
            .saturating_mul(self.height as usize)
            .saturating_mul(4)
    }
}

#[derive(Debug)]
pub struct Mp4Recorder {
    config: Mp4Config,
    output: PathBuf,
    child: Option<Child>,
    stdin: Option<ChildStdin>,
}

impl Mp4Recorder {
    pub fn ffmpeg_available() -> bool {
        Command::new(ffmpeg_bin())
            .arg("-version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|status| status.success())
            .unwrap_or(false)
    }

    pub fn start(output: impl AsRef<Path>, config: Mp4Config) -> io::Result<Self> {
        let output = output.as_ref().to_path_buf();
        if let Some(parent) = output.parent() {
            fs::create_dir_all(parent)?;
        }

        let mut child = Command::new(ffmpeg_bin())
            .arg("-hide_banner")
            .arg("-loglevel")
            .arg("error")
            .arg("-y")
            .arg("-f")
            .arg("rawvideo")
            .arg("-pix_fmt")
            .arg("rgba")
            .arg("-s")
            .arg(format!("{}x{}", config.width, config.height))
            .arg("-r")
            .arg(config.fps.to_string())
            .arg("-i")
            .arg("-")
            .arg("-an")
            .arg("-c:v")
            .arg("libx264")
            .arg("-preset")
            .arg("ultrafast")
            .arg("-crf")
            .arg("18")
            .arg("-pix_fmt")
            .arg("yuv420p")
            .arg(&output)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .spawn()?;

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| io::Error::new(io::ErrorKind::Other, "ffmpeg stdin was not piped"))?;

        Ok(Self {
            config,
            output,
            child: Some(child),
            stdin: Some(stdin),
        })
    }

    pub fn output(&self) -> &Path {
        &self.output
    }

    pub fn push_rgba_frame(&mut self, rgba: &[u8]) -> io::Result<()> {
        let expected = self.config.rgba_frame_len();
        if rgba.len() != expected {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("rgba frame len {} != expected {}", rgba.len(), expected),
            ));
        }

        let Some(stdin) = self.stdin.as_mut() else {
            return Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "ffmpeg stdin is closed",
            ));
        };
        stdin.write_all(rgba)
    }

    pub fn finish(&mut self) -> io::Result<()> {
        if let Some(stdin) = self.stdin.take() {
            drop(stdin);
        }

        if let Some(mut child) = self.child.take() {
            let status = child.wait()?;
            if !status.success() {
                return Err(io::Error::new(
                    io::ErrorKind::Other,
                    format!("ffmpeg exited with status {}", status),
                ));
            }
        }

        Ok(())
    }
}

impl Drop for Mp4Recorder {
    fn drop(&mut self) {
        let _ = self.finish();
    }
}

