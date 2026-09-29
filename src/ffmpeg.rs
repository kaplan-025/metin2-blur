use crate::blur::blur_region;
use crate::detect::{Cfg, Detector, Rect};
use std::collections::VecDeque;
use std::io::{self, Read, Write};
use std::path::Path;
#[cfg(target_os = "windows")]
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::Arc;

#[derive(Clone)]
pub struct Probe {
    pub w: usize,
    pub h: usize,
    pub fps: f64,
    pub duration: f64,
    pub frames: Option<u64>,
}

// Arac cozumleme sirasi: FFMPEG/FFPROBE env -> exe'nin yanindaki dosya -> PATH
pub fn tool(name: &str) -> String {
    let exe_suffix = if cfg!(target_os = "windows") { ".exe" } else { "" };
    if let Ok(v) = std::env::var(name.to_uppercase()) {
        if !v.is_empty() {
            return v;
        }
    }
    let file = format!("{name}{exe_suffix}");
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let cand = dir.join(&file);
            if cand.is_file() {
                return cand.to_string_lossy().to_string();
            }
        }
    }
    file
}

// Windows'ta tek exe: gomulu ffmpeg.exe'yi ilk calistirmada temp dizinine yaz.
pub fn ensure_embedded_ffmpeg() {
    #[cfg(target_os = "windows")]
    {
        use std::io::Write;
        if which_tool(&tool("ffmpeg")) {
            return;
        }
        let dir = std::env::var("LOCALAPPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|_| std::env::temp_dir())
            .join("metin2-blur");
        let _ = std::fs::create_dir_all(&dir);
        let dest = dir.join("ffmpeg.exe");
        if dest.is_file() {
            std::env::set_var("FFMPEG", &dest);
            return;
        }
        if let Ok(mut f) = std::fs::File::create(&dest) {
            if f.write_all(include_bytes!("../ffmpeg-win.exe")).is_ok() {
                let _ = f.flush();
                std::env::set_var("FFMPEG", &dest);
            }
        }
    }
    #[cfg(not(target_os = "windows"))]
    {}
}

// PATH ayiricisi platforma gore degisir (Windows ';'); std ayirici kullanilir.
pub fn which_tool(bin: &str) -> bool {
    std::env::var_os("PATH")
        .map(|p| {
            std::env::split_paths(&p)
                .any(|d| d.join(bin).is_file())
        })
        .unwrap_or(false)
}

// ffmpeg -i dosya  -> stderr'da cozunurluk/fps/sure. ffprobe'a gerek yok.
pub fn probe(path: &Path) -> io::Result<Probe> {
    let out = Command::new(tool("ffmpeg"))
        .args(["-hide_banner", "-i"])
        .arg(path)
        .output()?;
    let txt = String::from_utf8_lossy(&out.stderr).to_string();

    let duration = txt
        .split("Duration:")
        .nth(1)
        .and_then(|s| s.split(',').next())
        .map(|s| {
            let mut it = s.trim().split(':');
            let h: f64 = it.next().unwrap_or("0").trim().parse().unwrap_or(0.0);
            let m: f64 = it.next().unwrap_or("0").trim().parse().unwrap_or(0.0);
            let sec: f64 = it.next().unwrap_or("0").trim().parse().unwrap_or(0.0);
            h * 3600.0 + m * 60.0 + sec
        })
        .unwrap_or(0.0);

    let vline = txt
        .lines()
        .find(|l| l.contains("Video:"))
        .unwrap_or_default()
        .to_string();

    let (w, h) = vline
        .split_whitespace()
        .find_map(|t| {
            let (a, b) = t.split_once('x')?;
            if a.chars().all(|c| c.is_ascii_digit()) && b.chars().next()?.is_ascii_digit() {
                Some((a.parse().ok()?, b.split('[').next()?.parse().ok()?))
            } else {
                None
            }
        })
        .unwrap_or((0, 0));

    let fps = {
        let mut f = 30.0;
        let toks: Vec<&str> = vline.split_whitespace().collect();
        for (i, t) in toks.iter().enumerate() {
            if *t == "fps" && i > 0 {
                if let Ok(v) = toks[i - 1].parse::<f64>() {
                    f = v;
                }
            }
        }
        f
    };

    if w == 0 || h == 0 {
        return Err(io::Error::new(
            io::ErrorKind::Other,
            format!("video okunamadi: {}", path.display()),
        ));
    }
    Ok(Probe {
        w,
        h,
        fps,
        duration,
        frames: None,
    })
}

pub struct Msg {
    pub frame: u64,
    pub total: Option<u64>,
    pub boxes: usize,
    pub fps: f64,
    pub done: bool,
    pub error: Option<String>,
}

pub struct Job {
    pub input: String,
    pub output: String,
    pub cfg: Cfg,
    pub look_ahead: usize,
    pub hold: usize,
    pub extra_rects: Vec<Rect>,
}

fn kill(c: &mut Child) {
    let _ = c.kill();
    let _ = c.wait();
}

fn apply_blur(buf: &mut [u8], w: usize, h: usize, rects: &[Rect], radius: usize) {
    let pad = radius * 2;
    for r in rects {
        let x0 = r.0.saturating_sub(pad);
        let y0 = r.1.saturating_sub(pad);
        let x1 = (r.2 + pad).min(w);
        let y1 = (r.3 + pad).min(h);
        if x1 > x0 && y1 > y0 {
            blur_region(buf, w, h, (x0, y0, x1, y1), radius);
        }
    }
}

pub fn run(job: Job, cancel: Arc<AtomicBool>, tx: Sender<Msg>) -> io::Result<()> {
    let in_path = Path::new(&job.input);
    let p = probe(in_path)?;
    if p.w == 0 || p.h == 0 {
        return Err(io::Error::new(io::ErrorKind::Other, "video cozunemedi"));
    }
    let total = p
        .frames
        .filter(|n| *n > 0)
        .or(if p.duration > 0.0 {
            Some((p.duration * p.fps).round() as u64)
        } else {
            None
        });
    let (w, h) = (p.w, p.h);
    let fs = w * h * 3;

    let mut dec = Command::new(tool("ffmpeg"))
        .args(["-y", "-v", "error", "-nostdin", "-i"])
        .arg(in_path)
        .args(["-map", "0:v:0", "-f", "rawvideo", "-pix_fmt", "rgb24", "-"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;

    let mut enc = Command::new(tool("ffmpeg"))
        .args(["-y", "-v", "error", "-nostdin", "-f", "rawvideo", "-pix_fmt", "rgb24"])
        .args(["-s", &format!("{w}x{h}"), "-r", &format!("{:.6}", p.fps), "-i", "-"])
        .args(["-i"])
        .arg(in_path)
        .args(["-map", "0:v:0", "-map", "1:a?", "-c:a", "copy", "-shortest"])
        .args(["-c:v", "libx264", "-preset", "veryfast", "-crf", "18", "-pix_fmt", "yuv420p"])
        .arg(&job.output)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()?;

    let enc_err = Arc::new(std::sync::Mutex::new(String::new()));
    {
        let sink = enc_err.clone();
        let mut es = enc.stderr.take().unwrap();
        std::thread::spawn(move || {
            let mut buf = String::new();
            let mut b = [0u8; 1024];
            loop {
                match es.read(&mut b) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => buf.push_str(&String::from_utf8_lossy(&b[..n])),
                }
            }
            *sink.lock().unwrap() = buf;
        });
    }

    let mut dec_stdout = dec.stdout.take().unwrap();
    let mut enc_stdin = enc.stdin.take().unwrap();

    let mut det = Detector::new(w, h);
    let radius = job.cfg.blur_radius();
    let mut buf = vec![0u8; fs];
    let mut queue: VecDeque<(Vec<u8>, Vec<Rect>)> = VecDeque::new();
    let mut history: Vec<Vec<Rect>> = Vec::new();
    let mut n: u64 = 0;
    let t0 = std::time::Instant::now();

    let result = (|| -> io::Result<()> {
        loop {
            if cancel.load(Ordering::Relaxed) {
                return Err(io::Error::new(io::ErrorKind::Interrupted, "iptal edildi"));
            }
            buf.resize(fs, 0);
            match dec_stdout.read_exact(&mut buf[..fs]) {
                Ok(()) => {}
                Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => break,
                Err(e) => return Err(e),
            }
            let boxes = det.detect(&buf, &job.cfg);
            if let Some(path) = std::env::var_os("M2_PPM") {
                let want: u64 = std::env::var("M2_PPM_FRAME")
                    .ok()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(0);
                let paint_mask = std::env::var_os("M2_PPM_MASK").is_some();
                if n == want {
                    let mut img = buf.clone();
                    if paint_mask {
                        for i in 0..det.mask_ink.len() {
                            if det.mask_ink[i] == 1 {
                                let o = i * 3;
                                img[o] = (img[o] as u32 * 2 / 5) as u8;
                                img[o + 1] = 30;
                                img[o + 2] = 30;
                            }
                        }
                    }
                    for r in &boxes {
                        for (x, y) in [
                            (r.0, r.1), (r.2 - 1, r.1), (r.0, r.3 - 1), (r.2 - 1, r.3 - 1),
                            ((r.0 + r.2) / 2, r.1), ((r.0 + r.2) / 2, r.3 - 1),
                            (r.0, (r.1 + r.3) / 2), (r.2 - 1, (r.1 + r.3) / 2),
                        ] {
                            if x < w && y < h {
                                let o = (y * w + x) * 3;
                                img[o] = 0;
                                img[o + 1] = 255;
                                img[o + 2] = 0;
                            }
                        }
                    }
                    let mut f = std::fs::File::create(&path).unwrap();
                    use std::io::Write as _;
                    write!(f, "P6\n{w} {h}\n255\n").unwrap();
                    f.write_all(&img).unwrap();
                }
            }
            if std::env::var_os("M2_BOXES").is_some()
                && (n < 2 || n == std::env::var("M2_PPM_FRAME").ok().and_then(|v| v.parse::<u64>().ok()).unwrap_or(25))
            {
                eprintln!("kare {n}: {} kutu {boxes:?}", boxes.len());
                let (bw, bh) = (w / 100, h / 50);
                for by in 0..50 {
                    let mut row = String::new();
                    for bx in 0..100 {
                        let i = (by * bh) * w + bx * bw;
                        row.push(if det.con[i] == 1 { '#' } else if det.pal[i] == 1 { 'P' } else { '.' });
                    }
                    eprintln!("{row}");
                }
            }
            queue.push_back((buf.clone(), boxes));
            if queue.len() <= job.look_ahead {
                continue;
            }

            let (frame, own_boxes) = queue.pop_front().unwrap();
            let mut rects: Vec<Rect> = Vec::new();
            for (_, b) in queue.iter() {
                rects.extend(b.iter().copied());
            }
            for b in history.iter() {
                rects.extend(b.iter().copied());
            }
            rects.extend(job.extra_rects.iter().copied());
            let mut out = frame;
            apply_blur(&mut out, w, h, &rects, radius);
            enc_stdin.write_all(&out)?;

            let count = rects.len();
            history.push(own_boxes);
            if history.len() > job.hold {
                history.remove(0);
            }
            n += 1;
            let el = t0.elapsed().as_secs_f64().max(0.001);
            let _ = tx.send(Msg {
                frame: n,
                total,
                boxes: count,
                fps: n as f64 / el,
                done: false,
                error: None,
            });
        }

        while let Some((frame, own)) = queue.pop_front() {
            let mut rects: Vec<Rect> = Vec::new();
            for b in history.iter() {
                rects.extend(b.iter().copied());
            }
            rects.extend(own.iter().copied());
            rects.extend(job.extra_rects.iter().copied());
            let mut out = frame;
            apply_blur(&mut out, w, h, &rects, radius);
            enc_stdin.write_all(&out)?;
            n += 1;
        }
        Ok(())
    })();

    if result.is_err() {
        kill(&mut dec);
        kill(&mut enc);
        let mut err = result.err().map(|e| e.to_string()).unwrap_or_default();
        let log = enc_err.lock().unwrap().trim().to_string();
        if !log.is_empty() {
            err = format!("{err} | ffmpeg: {log}");
        }
        let _ = tx.send(Msg {
            frame: n,
            total,
            boxes: 0,
            fps: 0.0,
            done: true,
            error: Some(err),
        });
        return Ok(());
    }

    drop(enc_stdin);
    let st = enc.wait();
    let _ = dec.wait();
    let enc_log = enc_err.lock().unwrap().clone();
    let mut error = None;
    if let Ok(st) = &st {
        if !st.success() {
            error = Some(format!("ffmpeg kodladi: {}", enc_log.trim()));
        }
    }
    let _ = tx.send(Msg {
        frame: n,
        total,
        boxes: 0,
        fps: 0.0,
        done: true,
        error,
    });
    Ok(())
}

pub fn grab_frame(path: &Path, at: f64, w: usize, h: usize) -> io::Result<Vec<u8>> {
    let out = Command::new(tool("ffmpeg"))
        .args(["-y", "-v", "error", "-nostdin", "-ss", &format!("{at:.3}"), "-i"])
        .arg(path)
        .args(["-frames:v", "1", "-f", "rawvideo", "-pix_fmt", "rgb24", "-"])
        .stderr(Stdio::null())
        .output()?;
    if out.stdout.len() < w * h * 3 {
        return Err(io::Error::new(io::ErrorKind::Other, "kare alinamadi"));
    }
    Ok(out.stdout[..w * h * 3].to_vec())
}
