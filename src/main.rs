mod blur;
mod detect;
mod ffmpeg;

use detect::{Cfg, Mode, Rect};
use eframe::egui;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::{channel, Receiver};
use std::sync::Arc;

const VIDEO_EXT: [&str; 5] = ["mp4", "mkv", "avi", "webm", "mov"];

fn has_video_ext(p: &Path) -> bool {
    p.extension()
        .map(|e| {
            let e = e.to_string_lossy().to_lowercase();
            VIDEO_EXT.contains(&e.as_str())
        })
        .unwrap_or(false)
}

struct Picker {
    dir: PathBuf,
    show: bool,
}

struct Preview {
    tex: egui::TextureHandle,
    w: usize,
    h: usize,
    boxes: Vec<Rect>,
}

struct App {
    input: String,
    output: String,
    probe: Option<ffmpeg::Probe>,
    cfg: Cfg,
    font_auto: bool,
    look_ahead: usize,
    hold: usize,
    safe_on: [bool; 4],
    safe: [Rect; 4],
    roi: [usize; 4],
    draw_mode: bool,
    drag_from: Option<egui::Pos2>,
    drag_to: Option<egui::Pos2>,
    status: String,
    info: String,
    running: bool,
    cancel: Option<Arc<AtomicBool>>,
    rx: Option<Receiver<ffmpeg::Msg>>,
    last: Option<ffmpeg::Msg>,
    picker: Option<Picker>,
    preview: Option<Preview>,
    prev_at: f64,
    show_mask: bool,
}

impl App {
    fn new() -> Self {
        App {
            input: String::new(),
            output: String::new(),
            probe: None,
            cfg: Cfg::default(),
            font_auto: true,
            look_ahead: 3,
            hold: 6,
            safe_on: [false; 4],
            // x, y, genislik, yukseklik (yuzde): chat, orta isim seridi, minimap, tum ekran
            safe: [(0, 62, 55, 30), (0, 25, 100, 40), (78, 0, 22, 18), (0, 0, 100, 100)],
            roi: [0, 0, 100, 100],
            draw_mode: false,
            drag_from: None,
            drag_to: None,
            status: "Video sec".into(),
            info: String::new(),
            running: false,
            cancel: None,
            rx: None,
            last: None,
            picker: None,
            preview: None,
            prev_at: 1.0,
            show_mask: true,
        }
    }

    fn load(&mut self, path: &str) {
        let p = Path::new(path);
        match ffmpeg::probe(p) {
            Ok(pr) => {
                if self.font_auto {
                    let r = self.roi_pixels(pr.w, pr.h);
                    self.cfg.font_px = auto_font_px(pr.w, pr.h, r);
                }
                self.cfg.merge_r = (self.cfg.font_px * 0.6).round().max(1.0) as usize;
                self.input = path.to_string();
                self.output = path
                    .rsplit_once('.')
                    .map(|(s, _)| format!("{s}_bulanik.mp4"))
                    .unwrap_or_else(|| format!("{path}_bulanik.mp4"));
                self.info = format!(
                    "{}x{} · {:.2} fps · {:.1} sn{}",
                    pr.w,
                    pr.h,
                    pr.fps,
                    pr.duration,
                    pr.frames
                        .map(|n| format!(" · {n} kare"))
                        .unwrap_or_default()
                );
                self.status = "Video yuklendi".into();
                self.probe = Some(pr);
                self.preview = None;
            }
            Err(e) => self.status = format!("HATA: {e}"),
        }
    }

    fn start(&mut self) {
        if self.input.is_empty() {
            self.status = "Once video sec".into();
            return;
        }
        ffmpeg::ensure_embedded_ffmpeg();
        if !ffmpeg::which_tool(&ffmpeg::tool("ffmpeg")) {
            self.status = "HATA: ffmpeg bulunamadi (FFMPEG=/path/to/ffmpeg ile goster)".into();
            return;
        }
        let pr = match self.probe.clone() {
            Some(p) => p,
            None => {
                self.load(&self.input.clone());
                match self.probe.clone() {
                    Some(p) => p,
                    None => return,
                }
            }
        };
        let r = (self.roi[0], self.roi[1], self.roi[2], self.roi[3]);
        let mut extra = Vec::new();
        for i in 0..4 {
            if self.safe_on[i] {
                let (x, y, w, h) = self.safe[i];
                let fx = |v: f32| (v / 100.0 * pr.w as f32) as usize;
                let fy = |v: f32| (v / 100.0 * pr.h as f32) as usize;
                let (x0, y0) = (fx(x as f32), fy(y as f32));
                extra.push((x0, y0, fx(x as f32 + w as f32), fy(y as f32 + h as f32)));
            }
        }
        let mut job_cfg = self.cfg.clone();
        if r != (0, 0, 100, 100) {
            let (px, py) = (r.0 as f32 / 100.0 * pr.w as f32, r.1 as f32 / 100.0 * pr.h as f32);
            job_cfg.roi = Some((
                px as usize,
                py as usize,
                (px + r.2 as f32 / 100.0 * pr.w as f32) as usize,
                (py + r.3 as f32 / 100.0 * pr.h as f32) as usize,
            ));
        }
        let job = ffmpeg::Job {
            input: self.input.clone(),
            output: self.output.clone(),
            cfg: job_cfg,
            look_ahead: self.look_ahead,
            hold: self.hold,
            extra_rects: extra,
        };
        let (tx, rx) = channel();
        self.cancel = Some(Arc::new(AtomicBool::new(false)));
        let c = self.cancel.clone().unwrap();
        self.rx = Some(rx);
        self.last = None;
        self.running = true;
        self.status = "Isleniyor...".into();
        std::thread::spawn(move || {
            let _ = ffmpeg::run(job, c, tx);
        });
    }

    fn roi_pixels(&self, w: usize, h: usize) -> Option<(usize, usize, usize, usize)> {
        let (a, b, c, d) = (self.roi[0], self.roi[1], self.roi[2], self.roi[3]);
        if (a, b, c, d) == (0, 0, 100, 100) {
            return None;
        }
        let (px, py) = (a as f32 / 100.0 * w as f32, b as f32 / 100.0 * h as f32);
        Some((
            px as usize,
            py as usize,
            (px + c as f32 / 100.0 * w as f32) as usize,
            (py + d as f32 / 100.0 * h as f32) as usize,
        ))
    }

    fn roi_rect(&self) -> Option<Rect> {
        if self.probe.is_none() {
            return None;
        }
        let (a, b, c, d) = (self.roi[0], self.roi[1], self.roi[2], self.roi[3]);
        if (a, b, c, d) == (0, 0, 100, 100) {
            return None;
        }
        let pr = self.probe.as_ref().unwrap();
        let (px, py) = (a as f32 / 100.0 * pr.w as f32, b as f32 / 100.0 * pr.h as f32);
        Some((
            px as usize,
            py as usize,
            (px + c as f32 / 100.0 * pr.w as f32) as usize,
            (py + d as f32 / 100.0 * pr.h as f32) as usize,
        ))
    }

    fn stop(&mut self) {
        if let Some(c) = &self.cancel {
            c.store(true, std::sync::atomic::Ordering::Relaxed);
        }
    }

    fn make_preview(&mut self, ctx: &egui::Context) {
        let pr = match self.probe.clone() {
            Some(p) => p,
            None => return,
        };
        match ffmpeg::grab_frame(Path::new(&self.input), self.prev_at, pr.w, pr.h) {
            Ok(mut frame) => {
                let mut d = detect::Detector::new(pr.w, pr.h);
                let boxes = d.detect(&frame, &self.cfg);
                if self.show_mask {
                    let ink = &d.mask_ink;
                    for i in 0..ink.len() {
                        if ink[i] == 1 {
                            let o = i * 3;
                            frame[o] = 255;
                            frame[o + 1] = 40;
                            frame[o + 2] = 40;
                        }
                    }
                }
                let img = egui::ColorImage::from_rgb([pr.w, pr.h], &frame);
                let tex = ctx.load_texture("prev", img, egui::TextureOptions::LINEAR);
                self.preview = Some(Preview {
                    tex,
                    w: pr.w,
                    h: pr.h,
                    boxes,
                });
                self.status = format!("Onizleme: {} kutu", self.preview.as_ref().unwrap().boxes.len());
            }
            Err(e) => self.status = format!("Onizleme hatasi: {e}"),
        }
    }
}

// Metin2 yazi 12 px @768. ROI verildiyse oyuncunun gorunur yuksekligi kullanilir
// (masaustu kaydinda 415 px oyun penceresi -> 6.5 px). ROI yoksa karenin yuksekligi:
// 1280x720 -> 11, dikey 720x1280 -> 20. min(w,h) denemesi yapildi, 720x1280
// icin gercek yazi ~11 px olmasina ragmen kotu sonuc verdi (olcum: bilesen sayisi ve
// bulanik alan) -- yukseklik formulu korundu.
fn auto_font_px(_w: usize, h: usize, roi: Option<(usize, usize, usize, usize)>) -> f32 {
    let ref_h = match roi {
        Some((_x0, y0, _x1, y1)) => (y1.saturating_sub(y0)) as f32,
        None => h as f32,
    };
    (12.0 * ref_h / 768.0).clamp(4.0, 40.0)
}

fn usage_exit() -> ! {
    eprintln!(
        "kullanim: metin2-blur --cli <video> [cikti] [hassasiyet] [font_px] [roi] [mod] [bolgeler]\n  roi: x,y,g,y yuzde (0,0,100,100 = tum ekran)\n  mod: ikisi | palet | yapi\n  bolgeler: x,y,g,y;x,y,g,y yuzde -> her zaman bulaniklasir"
    );
    std::process::exit(2);
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if let Some(rx) = &self.rx {
            while let Ok(m) = rx.try_recv() {
                if m.done {
                    self.running = false;
                    if let Some(e) = &m.error {
                        self.status = format!("HATA: {e}");
                    } else {
                        self.status = format!("Bitti · {}", self.output);
                    }
                }
                self.last = Some(m);
            }
        }

        egui::TopBottomPanel::top("ust").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.heading("Metin2 Blur");
                ui.label("videodaki tum yazilari bulaniklastirir (Metin2 yazi stiline gore)");
                if ui.link("videoyu bırak (sürükle)").clicked() {
                    self.status = "Pencereye video dosyasi birak".into();
                }
            });
        });

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label("Video:");
                ui.add(
                    egui::TextEdit::singleline(&mut self.input)
                        .hint_text("/home/kali/videos/metin2.mp4")
                        .desired_width(340.0),
                );
                if self.font_auto {
                if let Some(pr) = self.probe.clone() {
                    let want = auto_font_px(pr.w, pr.h, self.roi_pixels(pr.w, pr.h));
                    if (want - self.cfg.font_px).abs() > 0.6 {
                        self.cfg.font_px = want;
                        self.cfg.merge_r = (want * 0.6).round().max(1.0) as usize;
                    }
                }
            }
            if ui.button("Yukle").clicked() {
                    let p = self.input.clone();
                    self.load(&p);
                }
                if ui.button("Sec...").clicked() {
                    let dir = if self.input.is_empty() {
                        std::env::var("HOME").unwrap_or_else(|_| "/".into())
                    } else {
                        Path::new(&self.input)
                            .parent()
                            .map(|p| p.to_string_lossy().to_string())
                            .unwrap_or_else(|| "/".into())
                    };
                    self.picker = Some(Picker {
                        dir: PathBuf::from(dir),
                        show: true,
                    });
                }
            });
            ui.label(format!("{} — {}", self.info, self.status));

            ui.separator();
            ui.horizontal(|ui| {
                ui.label("Islem alani (ROI):");
                ui.label("tum ekran");
                let r = &mut self.roi;
                ui.add(egui::DragValue::new(&mut r[0]).range(0.0..=100.0).prefix("x"));
                ui.add(egui::DragValue::new(&mut r[1]).range(0.0..=100.0).prefix("y"));
                ui.add(egui::DragValue::new(&mut r[2]).range(1.0..=100.0).prefix("g"));
                ui.add(egui::DragValue::new(&mut r[3]).range(1.0..=100.0).prefix("y"));
                if ui.button("tum ekran").clicked() {
                    self.roi = [0, 0, 100, 100];
                }
            });
            ui.horizontal(|ui| {
                ui.checkbox(&mut self.draw_mode, "Onizlemede bolge ciz (her zaman bulaniklasir)");
                if ui.button("bolgeleri temizle").clicked() {
                    self.safe_on = [false; 4];
                    self.safe = [(0, 0, 1, 1); 4];
                }
            });

            ui.separator();
            egui::CollapsingHeader::new("Guvenli bolgeler (algilama kacirirsa her zaman bulaniklasir)")
                .default_open(false)
                .show(ui, |ui| {
                    ui.label("Yuzde olarak x, y, genislik, yukseklik. Ornek: chat 0,62,55,30");
                    for i in 0..4 {
                        ui.horizontal(|ui| {
                            let mut on = self.safe_on[i];
                            if ui.checkbox(&mut on, format!("#{}", i + 1)).changed() {
                                self.safe_on[i] = on;
                            }
                            let r = &mut self.safe[i];
                            ui.add(egui::DragValue::new(&mut r.0).range(0.0..=100.0).prefix("x"));
                            ui.add(egui::DragValue::new(&mut r.1).range(0.0..=100.0).prefix("y"));
                            ui.add(egui::DragValue::new(&mut r.2).range(1.0..=100.0).prefix("g"));
                            ui.add(egui::DragValue::new(&mut r.3).range(1.0..=100.0).prefix("y"));
                        });
                    }
                });

            egui::Grid::new("ayar").num_columns(4).spacing([12.0, 4.0]).show(ui, |ui| {
                ui.label("Hassasiyet");
                ui.add(
                    egui::Slider::new(&mut self.cfg.sensitivity, 0.0..=1.0)
                        .text("0 = katı, 1 = her sey"),
                );
                ui.label("Mod");
                egui::ComboBox::from_id_salt("mod")
                    .selected_text(match self.cfg.mode {
                        Mode::Palette => "sadece Metin2 renkleri",
                        Mode::Structure => "sadece yapi (ozgun renk)",
                        Mode::Both => "ikisi (varsayilan)",
                    })
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut self.cfg.mode, Mode::Both, "ikisi (varsayilan)");
                        ui.selectable_value(&mut self.cfg.mode, Mode::Palette, "sadece Metin2 renkleri");
                        ui.selectable_value(&mut self.cfg.mode, Mode::Structure, "sadece yapi (ozgun renk)");
                    });
                ui.end_row();

                ui.label("Font yuksekligi (px)");
                if ui.add(egui::Slider::new(&mut self.cfg.font_px, 4.0..=60.0).text("12px @768 -> 17px @1080")).changed() {
                    self.font_auto = false;
                }
                ui.checkbox(&mut self.font_auto, "videodan hesapla");
                ui.end_row();

                ui.label("Satir birlestirme (px)");
                ui.add(egui::Slider::new(&mut self.cfg.merge_r, 0..=40));
                ui.label("Bulaniklik gucu");
                ui.add(egui::Slider::new(&mut self.cfg.blur_scale, 0.5..=3.0).text("1.6 = varsayilan"));
                ui.label("Kutu genisletme (px)");
                ui.add(egui::Slider::new(&mut self.cfg.margin_px, 0.0..=20.0));
                ui.end_row();

                ui.label("Onizleme karti durdurma (kare)");
                ui.add(egui::Slider::new(&mut self.hold, 0..=30));
                ui.label("Ileri bakis (kare)");
                ui.add(egui::Slider::new(&mut self.look_ahead, 0..=10));
                ui.end_row();
            });

            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(!self.running, egui::Button::new("Baslat").min_size(egui::vec2(120.0, 30.0)))
                    .clicked()
                {
                    self.start();
                }
                if ui
                    .add_enabled(self.running, egui::Button::new("Iptal").min_size(egui::vec2(90.0, 30.0)))
                    .clicked()
                {
                    self.stop();
                }
                if ui.button("Onizle").clicked() {
                    self.make_preview(ctx);
                }
                ui.checkbox(&mut self.show_mask, "maskeyi kirmizi goster");
                ui.label(format!("kare {:.2} sn", self.prev_at));
                if ui.add(egui::Slider::new(&mut self.prev_at, 0.0..=600.0)).changed()
                    && self.preview.is_some()
                {
                    self.make_preview(ctx);
                }
            });
            ui.add(
                egui::TextEdit::singleline(&mut self.output)
                    .hint_text("cikti dosyasi")
                    .desired_width(f32::INFINITY),
            );

            if let Some(m) = &self.last {
                let f = match m.total {
                    Some(t) if t > 0 => (m.frame as f32 / t as f32).clamp(0.0, 1.0),
                    _ => 0.0,
                };
                ui.add(egui::ProgressBar::new(f).desired_width(f32::INFINITY));
                let eta = match m.total {
                    Some(t) if m.frame > 0 && m.fps > 0.0 => {
                        format!(" · kalan ~{:.0} sn", (t - m.frame) as f64 / m.fps)
                    }
                    _ => String::new(),
                };
                ui.label(format!(
                    "kare {}{} · {:.1} kare/sn · {} kutu{eta}",
                    m.frame,
                    m.total.map(|t| format!("/{t}")).unwrap_or_default(),
                    m.fps,
                    m.boxes
                ));
            }

            if let Some(p) = &self.preview {
                let avail = ui.available_width();
                let s = (avail / p.w as f32).min(1.0).max(0.05);
                let size = egui::vec2(p.w as f32 * s, p.h as f32 * s);
                let sense = if self.draw_mode {
                    egui::Sense::click_and_drag()
                } else {
                    egui::Sense::hover()
                };
                let (rect, resp) = ui.allocate_exact_size(size, sense);
                let painter = ui.painter();
                painter.image(
                    p.tex.id(),
                    rect,
                    egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                    egui::Color32::WHITE,
                );
                for b in &p.boxes {
                    let r = egui::Rect::from_min_max(
                        egui::pos2(rect.left() + b.0 as f32 * s, rect.top() + b.1 as f32 * s),
                        egui::pos2(rect.left() + b.2 as f32 * s, rect.top() + b.3 as f32 * s),
                    );
                    painter.rect_stroke(
                        r,
                        0.0,
                        egui::Stroke::new(1.0_f32, egui::Color32::from_rgb(0, 255, 120)),
                        egui::StrokeKind::Middle,
                    );
                }
                if let Some(ro) = self.roi_rect() {
                    let r = egui::Rect::from_min_max(
                        egui::pos2(rect.left() + ro.0 as f32 * s, rect.top() + ro.1 as f32 * s),
                        egui::pos2(rect.left() + ro.2 as f32 * s, rect.top() + ro.3 as f32 * s),
                    );
                    painter.rect_stroke(
                        r,
                        0.0,
                        egui::Stroke::new(2.0_f32, egui::Color32::from_rgb(255, 200, 0)),
                        egui::StrokeKind::Middle,
                    );
                }
                for i in 0..4 {
                    if !self.safe_on[i] {
                        continue;
                    }
                    let b = self.safe[i];
                    let r = egui::Rect::from_min_max(
                        egui::pos2(
                            rect.left() + b.0 as f32 / 100.0 * p.w as f32 * s,
                            rect.top() + b.1 as f32 / 100.0 * p.h as f32 * s,
                        ),
                        egui::pos2(
                            rect.left() + (b.0 + b.2) as f32 / 100.0 * p.w as f32 * s,
                            rect.top() + (b.1 + b.3) as f32 / 100.0 * p.h as f32 * s,
                        ),
                    );
                    painter.rect_filled(r, 0.0, egui::Color32::from_rgb(255, 60, 0).gamma_multiply(0.25));
                    painter.rect_stroke(
                        r,
                        0.0,
                        egui::Stroke::new(1.5_f32, egui::Color32::from_rgb(255, 120, 0)),
                        egui::StrokeKind::Middle,
                    );
                }
                if self.draw_mode {
                    if resp.drag_started() {
                        self.drag_from = resp.interact_pointer_pos;
                    }
                    if resp.dragged() {
                        self.drag_to = resp.interact_pointer_pos;
                    }
                    if let (Some(a), Some(b)) = (self.drag_from, self.drag_to) {
                        painter.rect_stroke(
                            egui::Rect::from_min_max(a, b),
                            0.0,
                            egui::Stroke::new(1.5_f32, egui::Color32::WHITE),
                            egui::StrokeKind::Middle,
                        );
                    }
                    if resp.drag_stopped() {
                        if let (Some(a), Some(b)) = (self.drag_from.take(), self.drag_to.take()) {
                            let to_pct = |v: f32, len: f32| {
                                (((v - if a.x <= b.x { a.x } else { b.x }) / len) * 100.0)
                                    .clamp(0.0, 100.0) as usize
                            };
                            let (x, y) = (
                                to_pct(a.x.min(b.x), size.x),
                                to_pct(a.y.min(b.y), size.y),
                            );
                            let (w, h) = (
                                to_pct((a.x - b.x).abs(), size.x).max(1),
                                to_pct((a.y - b.y).abs(), size.y).max(1),
                            );
                            if let Some(slot) = (0..4).find(|i| !self.safe_on[*i]) {
                                self.safe[slot] = (x, y, w, h);
                                self.safe_on[slot] = true;
                            } else {
                                self.status = "4 bolge dolu, once birini kapat".into();
                            }
                        }
                    }
                }
            }
        });

        if self.picker.is_some() {
            let mut done = false;
            let mut p = self.picker.take().unwrap();
            egui::Window::new("Video sec")
                .open(&mut p.show)
                .default_size([560.0, 420.0])
                .show(ctx, |ui| {
                    ui.horizontal(|ui| {
                        if ui.button("..").clicked() {
                            if let Some(up) = p.dir.parent() {
                                p.dir = up.to_path_buf();
                            }
                        }
                        if ui.button("Ana sayfa").clicked() {
                            p.dir = PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| "/".into()));
                        }
                        ui.label(p.dir.to_string_lossy().to_string());
                    });
                    ui.separator();
                    let mut entries: Vec<_> = std::fs::read_dir(&p.dir)
                        .map(|it| it.filter_map(|e| e.ok()).map(|e| e.path()).collect())
                        .unwrap_or_default();
                    entries.sort();
                    egui::ScrollArea::vertical().show(ui, |ui| {
                        for e in entries {
                            let name = e
                                .file_name()
                                .map(|n| n.to_string_lossy().to_string())
                                .unwrap_or_default();
                            if e.is_dir() {
                                if ui.selectable_label(false, format!("{name}/")).clicked() {
                                    p.dir = e;
                                }
                            } else if has_video_ext(&e) {
                                if ui.selectable_label(false, &name).clicked() {
                                    let s = e.to_string_lossy().to_string();
                                    self.load(&s);
                                    done = true;
                                }
                            }
                        }
                    });
                });
            if !done {
                self.picker = Some(p);
            }
        }

        // is surerken surekli yeniden ciz: ilerleme mesajlari ancak update cagirildikca okunur
        if self.running {
            ctx.request_repaint();
        }

        if let Some(drop) = ctx.input(|i| i.raw.dropped_files.first().cloned()) {
            if let Some(path) = drop.path {
                let s = path.to_string_lossy().to_string();
                self.load(&s);
            }
        }
    }
}

fn main() -> eframe::Result {
    ffmpeg::ensure_embedded_ffmpeg();
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(|s| s == "--cli").unwrap_or(false) {
        // toplu islem: metin2-blur --cli <video> [cikti] [hassasiyet 0..1] [font_px]
        let input = args.get(2).cloned().unwrap_or_else(|| usage_exit());
        let output = args.get(3).cloned().unwrap_or_else(|| {
            input
                .rsplit_once('.')
                .map(|(s, _)| format!("{s}_bulanik.mp4"))
                .unwrap_or_else(|| format!("{input}_bulanik.mp4"))
        });
        let mut cfg = Cfg::default();
        if let Some(s) = args.get(4) {
            cfg.sensitivity = s.parse().unwrap_or(0.55);
        }
        if let Some(f) = args.get(5) {
            cfg.font_px = f.parse().unwrap_or(cfg.font_px);
        } else if let Ok(pr) = ffmpeg::probe(Path::new(&input)) {
            cfg.font_px = auto_font_px(pr.w, pr.h, None);
        }
        if let Ok(b) = std::env::var("M2_BLUR") {
            cfg.blur_scale = b.parse().unwrap_or(cfg.blur_scale);
        }
        let mf: f32 = std::env::var("M2_MERGE")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(0.6);
        cfg.merge_r = (cfg.font_px * mf).round().max(1.0) as usize;
        match args.get(7).map(|s| s.as_str()) {
            Some("palet") => cfg.mode = Mode::Palette,
            Some("yapi") => cfg.mode = Mode::Structure,
            _ => {}
        }
        // islem alani: "x,y,yuzdeGenislik,yuzdeYukseklik" (0,0,100,100 = tum ekran)
        if let Some(r) = args.get(6) {
            let v: Vec<f32> = r.split(',').filter_map(|x| x.parse().ok()).collect();
            if v.len() == 4 {
                let pr = ffmpeg::probe(Path::new(&input)).ok();
                if let Some(pr) = pr {
                    let (px, py) = (v[0] / 100.0 * pr.w as f32, v[1] / 100.0 * pr.h as f32);
                    let pw = v[2] / 100.0 * pr.w as f32;
                    let ph = v[3] / 100.0 * pr.h as f32;
                    cfg.roi = Some((
                        px as usize,
                        py as usize,
                        (px + pw) as usize,
                        (py + ph) as usize,
                    ));
                }
            }
        }
        eprintln!(
            "font_px={:.1} merge={} hassasiyet={:.2} esik={} palet_tol={} mod={:?} roi={:?}",
            cfg.font_px,
            cfg.merge_r,
            cfg.sensitivity,
            cfg.contrast_thr(),
            cfg.palette_tol(),
            cfg.mode,
            cfg.roi
        );
        // guvenli bolgeler: "x,y,g,y;x,y,g,y" (yuzde). Algilama kacirsa burasi her zaman bulaniklasir.
        let mut extra = Vec::new();
        if let Some(list) = args.get(9) {
            if let Ok(pr) = ffmpeg::probe(Path::new(&input)) {
                for r in list.split(';').filter(|s| !s.trim().is_empty()) {
                    let v: Vec<f32> = r.split(',').filter_map(|x| x.parse().ok()).collect();
                    if v.len() == 4 {
                        let (px, py) = (v[0] / 100.0 * pr.w as f32, v[1] / 100.0 * pr.h as f32);
                        extra.push((
                            px as usize,
                            py as usize,
                            (px + v[2] / 100.0 * pr.w as f32) as usize,
                            (py + v[3] / 100.0 * pr.h as f32) as usize,
                        ));
                    }
                }
            }
        }
        let (tx, rx) = channel();
        let job = ffmpeg::Job {
            input,
            output: output.clone(),
            cfg,
            look_ahead: 3,
            hold: 6,
            extra_rects: extra,
        };
        let erx = tx.clone();
        std::thread::spawn(move || {
            if let Err(e) = ffmpeg::run(job, Arc::new(AtomicBool::new(false)), tx) {
                let _ = erx.send(ffmpeg::Msg {
                    frame: 0,
                    total: None,
                    boxes: 0,
                    fps: 0.0,
                    done: true,
                    error: Some(e.to_string()),
                });
            }
        });
        let mut last = String::new();
        while let Ok(m) = rx.recv() {
            let s = format!(
                "kare {}{} · {:.1} fps · {} kutu",
                m.frame,
                m.total.map(|t| format!("/{t}")).unwrap_or_default(),
                m.fps,
                m.boxes
            );
            if s != last {
                println!("{s}");
                last = s;
            }
            if m.done {
                if let Some(e) = m.error {
                    eprintln!("HATA: {e}");
                    std::process::exit(1);
                }
                println!("bitti: {output}");
                return Ok(());
            }
        }
        return Ok(());
    }

    let opts = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([1100.0, 820.0]),
        ..Default::default()
    };
    eframe::run_native(
        "Metin2 Blur",
        opts,
        Box::new(|_cc| Ok(Box::new(App::new()))),
    )
}
