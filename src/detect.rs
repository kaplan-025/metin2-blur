use crate::blur::{dilate_h, morph_gray};

pub type Rect = (usize, usize, usize, usize);

const STAB_K: usize = 12;

// Metin2 client kaynak kodundaki sabit yazi renkleri:
//   PythonTextTail.cpp  c_TextTail_Player_Color(1,1,1) / Monster(1,0,0) / Item(1,1,1)
//                       / Chat(1,1,1) / Info(1,0.785,0.785) / Guild 0xFFEFD3FF
//   PythonTextTail.cpp  sahip (1,1,0), level/guild: 959697 FFFFFF 00FF7F 00FF00 00CCFF
//                       0000FF DA70D6 C67171 00FFFF FFCC00 FF4500 CD661D FFC125 888888
// GrpFontTexture.cpp:274 glyph maskesi 1-bit + A4R4G4B4 modulate -> ekranda tek duz renk.
pub const PALETTE: [(u8, u8, u8); 17] = [
    (255, 255, 255),
    (255, 0, 0),
    (255, 255, 0),
    (255, 200, 200),
    (239, 211, 255),
    (205, 102, 29),
    (255, 193, 37),
    (0, 255, 0),
    (0, 255, 127),
    (0, 204, 255),
    (0, 0, 255),
    (0, 255, 255),
    (255, 204, 0),
    (255, 69, 0),
    (218, 112, 214),
    (199, 113, 113),
    (149, 150, 151),
];

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    Palette,
    Structure,
    Both,
}

#[derive(Clone)]
pub struct Cfg {
    pub sensitivity: f32,
    pub mode: Mode,
    pub font_px: f32,
    pub merge_r: usize,
    pub margin_px: f32,
    pub max_line_h: f32,
    pub min_density: f32,
    pub blur_scale: f32,
    pub roi: Option<Rect>,
    pub stable_min: f32,
}

impl Default for Cfg {
    fn default() -> Self {
        Cfg {
            sensitivity: 0.55,
            mode: Mode::Both,
            font_px: 17.0,
            merge_r: 10,
            margin_px: 3.0,
            max_line_h: 3.2,
            min_density: 0.03,
            blur_scale: 1.6,
            roi: None,
            stable_min: 0.0,
        }
    }
}

impl Cfg {
    pub fn contrast_thr(&self) -> i32 {
        // 28..100: kucuk ve dusuk kontrastli yazi (masaustu kaydi, 8 px) icin gerekiyor
        (100.0 - 72.0 * self.sensitivity.clamp(0.0, 1.0)) as i32
    }
    pub fn palette_tol(&self) -> i32 {
        (34.0 - 20.0 * self.sensitivity.clamp(0.0, 1.0)) as i32
    }
    // Kutu icin bulaniklik yaricapı. Kucuk/dusuk kontrastli yazida genis yaricap
    // fark yaratiyor (olcum: ayni kutuda 4.7 px -> %80 okunur, 9 px -> %38).
    pub fn blur_radius(&self) -> usize {
        (self.font_px * 0.45 * self.blur_scale).round().clamp(3.0, 24.0) as usize
    }
}

pub struct Detector {
    w: usize,
    h: usize,
    ch: [Vec<u8>; 3],
    thin: [Vec<u8>; 3],
    pub pal: Vec<u8>,
    pub con: Vec<u8>,
    ink: Vec<u8>,
    opened: Vec<u8>,
    merged: Vec<u8>,
    parent: Vec<u32>,
    pal_lut: Vec<u8>,
    pal_lut_tol: i32,
    stability: Vec<u8>,
    hist: Vec<u8>,
    hist_at: usize,
    seen: usize,
    pub mask_ink: Vec<u8>,
}

impl Detector {
    pub fn new(w: usize, h: usize) -> Self {
        let n = w * h;
        Detector {
            w,
            h,
            ch: [vec![0; n], vec![0; n], vec![0; n]],
            thin: [vec![0; n], vec![0; n], vec![0; n]],
            pal: vec![0; n],
            con: vec![0; n],
            ink: vec![0; n],
            opened: vec![0; n],
            merged: vec![0; n],
            parent: vec![0; n],
            pal_lut: vec![0; 32768],
            pal_lut_tol: -1,
            stability: vec![0; n],
            hist: vec![0; n * STAB_K],
            hist_at: 0,
            seen: 0,
            mask_ink: vec![0; n],
        }
    }

    // 5-bit kova basina 1 bayt: piksel basina 17 palet karsilastirmasi yerine tek dizinleme
    // ponytail: kova 8 birim, bu yuzden tolerans 10'un altinda guvenilmez (bulaniklastirmada sorun degil)
    fn build_lut(&mut self, tol: i32) {
        if self.pal_lut_tol == tol {
            return;
        }
        for r in 0..32i32 {
            for g in 0..32i32 {
                for b in 0..32i32 {
                    let (r, g, b) = (r * 8 + 4, g * 8 + 4, b * 8 + 4);
                    let mut hit = 0u8;
                    for p in PALETTE {
                        let dr = r - p.0 as i32;
                        let dg = g - p.1 as i32;
                        let db = b - p.2 as i32;
                        if dr * dr + dg * dg + db * db <= tol * tol {
                            hit = 1;
                            break;
                        }
                    }
                    self.pal_lut[((r as usize >> 3) << 10) | ((g as usize >> 3) << 5) | (b as usize >> 3)] = hit;
                }
            }
        }
        self.pal_lut_tol = tol;
    }

    // con | pal maskesinde bir kenar satirinda piksel var mi
    fn edge_has(&self, a: usize, b0: usize, b1: usize, col: bool) -> bool {
        (b0..b1).any(|b| {
            let i = if col { b * self.w + a } else { a * self.w + b };
            self.con[i] == 1 || self.pal[i] == 1
        })
    }

    fn uf_find(&mut self, mut a: u32) -> u32 {
        while self.parent[a as usize] != a {
            self.parent[a as usize] = self.parent[self.parent[a as usize] as usize];
            a = self.parent[a as usize];
        }
        a
    }

    fn uf_union(&mut self, a: u32, b: u32) {
        let (ra, rb) = (self.uf_find(a), self.uf_find(b));
        if ra != rb {
            self.parent[rb as usize] = ra;
        }
    }

    pub fn detect(&mut self, frame: &[u8], cfg: &Cfg) -> Vec<Rect> {
        let (w, h, n) = (self.w, self.h, self.w * self.h);
        let mut _t = pnow();
        // kanal ayirma asagida is parcaciklarinin icinde yapiliyor (ayri gecis
        // kare basina 9-13 ms tutuyordu)
        padd(P_SPLIT, _t);
        let thr = cfg.contrast_thr();
        let tol = cfg.palette_tol() as i32;
        let use_pal = cfg.mode != Mode::Structure;
        let use_str = cfg.mode != Mode::Palette;
        if use_pal {
            self.build_lut(tol);
        }

        // ince yapi operatoru: gri tonlamali acma (erosion->dilation) + top-hat/black-hat.
        // Acma, kalin parlakliklari (gok, panel) siler ve glif aralarini korur; 4:2:0 +
        // H.264 palet rengini bozsa bile yazi cekirdegi tepki verir.
        let r_open = (cfg.font_px * 0.18).round().max(1.0) as usize;
        _t = pnow();
        // 3 kanal birbirinden bagimsiz -> 3 is parcacigi. (Tek cekirdekte morfoloji
        // kare basina 89 ms idi, bu makine 4 cekirdekli bir Ryzen.)
        let (chp, thin) = (&mut self.ch, &mut self.thin);
        let mut planes: [Vec<u8>; 3] = [
            std::mem::take(&mut chp[0]),
            std::mem::take(&mut chp[1]),
            std::mem::take(&mut chp[2]),
        ];
        let mut dsts: [Vec<u8>; 3] = [
            std::mem::take(&mut thin[0]),
            std::mem::take(&mut thin[1]),
            std::mem::take(&mut thin[2]),
        ];
        let mut outs: [(Vec<u8>, Vec<u8>); 3] = std::array::from_fn(|_| (Vec::new(), Vec::new()));
        std::thread::scope(|sc| {
            let mut hs = Vec::new();
            for c in 0..3 {
                let (mut src, mut dst) =
                    (std::mem::take(&mut planes[c]), std::mem::take(&mut dsts[c]));
                hs.push(sc.spawn(move || {
                    // kanal ayirma da burada: ayri bir gecis kare basina 13 ms tutuyordu
                    for i in 0..n {
                        src[i] = frame[i * 3 + c];
                    }
                    let ero = morph_gray(&src, w, h, r_open, false);
                    let opn = morph_gray(&ero, w, h, r_open, true);
                    dst.fill(0);
                    for i in 0..n {
                        let v = src[i].saturating_sub(opn[i]).max(ero[i].saturating_sub(src[i]));
                        if v > dst[i] {
                            dst[i] = v;
                        }
                    }
                    (src, dst)
                }));
            }
            for (c, hh) in hs.into_iter().enumerate() {
                outs[c] = hh.join().unwrap();
            }
        });
        for c in 0..3 {
            chp[c] = std::mem::take(&mut outs[c].0);
            thin[c] = std::mem::take(&mut outs[c].1);
        }
        padd(P_MORPH, _t);
        _t = pnow();
        // dal yok: mod secimini son anda maskeyle zorluyoruz, boylece dongu
        // vektorlestirilebiliyor. Satir seritlerine bolunup paralel isleniyor
        // (720p'de 7 ms -> ~2 ms).
        let (pal_on, str_on) = (use_pal as u8, use_str as u8);
        {
            let (c0, c1, c2) = (&self.ch[0], &self.ch[1], &self.ch[2]);
            let (t0, t1, t2) = (&self.thin[0], &self.thin[1], &self.thin[2]);
            let lut = &self.pal_lut[..];
            let rows = (h + 3) / 4;
            let groups: Vec<(&mut [u8], &mut [u8], &mut [u8])> = self
                .pal
                .chunks_mut(rows * w)
                .zip(self.con.chunks_mut(rows * w))
                .zip(self.ink.chunks_mut(rows * w))
                .map(|((a, b), c)| (a, b, c))
                .collect();
            std::thread::scope(|sc| {
                for (k, (pc, cc, ic)) in groups.into_iter().enumerate() {
                    // dilim indeksi yerel: kaynak dizilerdeki taban k*rows*w
                    let base = k * rows * w;
                    sc.spawn(move || {
                        for i in 0..pc.len() {
                            let j = base + i;
                            let (r, g, b) = (c0[j], c1[j], c2[j]);
                            let p = lut[((r as usize >> 3) << 10)
                                | ((g as usize >> 3) << 5)
                                | (b as usize >> 3)]
                                & pal_on;
                            let c = (((t0[j] as i32) >= thr) as u8
                                | ((t1[j] as i32) >= thr) as u8
                                | ((t2[j] as i32) >= thr) as u8)
                                & str_on;
                            pc[i] = p;
                            cc[i] = c;
                            ic[i] = p | c;
                        }
                    });
                }
            });
        }

        // islem alani disini sifirla (masaustu kaydi: duvar kagidi, arac penceresi, gorev cubugu)
        if let Some(r) = cfg.roi {
            for y in 0..h {
                let base = y * w;
                if y < r.1 {
                    self.ink[base..base + w].fill(0);
                } else if y >= r.3 {
                    self.ink[base..base + w].fill(0);
                } else {
                    let (a, b) = (r.0.min(w), r.2.min(w));
                    if a > 0 {
                        self.ink[base..base + a].fill(0);
                    }
                    if b < w {
                        self.ink[base + b..base + w].fill(0);
                    }
                }
            }
        }

        // istatistik (alan + satir ici gecis) iki kanaldan daha "yazi gibi" olanini kullanir
        let two_stat = use_pal && use_str;

        // Zaman kararliligi: 3B sahne kayar, yazi sabit kalir. Gercek kayan pencere
        // (STAB_K karelik "bu pikel son STAB_K karede ne kadar ink idi" sayaci).
        padd(P_MASK, _t);
        _t = pnow();
        // Varsayilan KAPALI: gecmeyi tutmak kare basina 8 ms tutuyordu ve
        // yeni/hareketli yaziyi olduruyordu. Acikken de ayni yol.
        let use_stab = cfg.stable_min > 0.0;
        if use_stab {
            let hb = self.hist_at * n;
            for i in 0..n {
                let old = self.hist[hb + i];
                let ink = self.ink[i];
                self.hist[hb + i] = ink;
                self.stability[i] = self.stability[i] + ink - old;
            }
            self.hist_at = (self.hist_at + 1) % STAB_K;
        }
        self.seen += 1;

        padd(P_STAB, _t);
        _t = pnow();
        self.mask_ink.copy_from_slice(&self.ink);

        self.opened.copy_from_slice(&self.ink);

        // yazi satirini birlestir: SADECE yatay, sozcuk aralari kapanir, ustteki satir tasmaz
        self.merged = dilate_h(&self.opened, w, h, cfg.merge_r);

        // bilesen etiketleme: satir run'lari uzerinden (piksel basina 9 komsu yerine run karsilastirmasi)
        padd(P_MERGE, _t);
        _t = pnow();
        let mut prev: Vec<(u32, u32, u32)> = Vec::new();
        let mut cur: Vec<(u32, u32, u32)> = Vec::new();
        let mut all: Vec<(u32, u32, u32, u32)> = Vec::new();
        let mut next = 1u32;
        for y in 0..h {
            cur.clear();
            let base = y * w;
            let mut x = 0usize;
            while x < w {
                if self.merged[base + x] == 1 {
                    let start = x;
                    while x < w && self.merged[base + x] == 1 {
                        x += 1;
                    }
                    let mut label = 0u32;
                    for &(s0, e0, l0) in prev.iter() {
                        if (s0 as usize) < x && (e0 as usize) > start {
                            if label == 0 {
                                label = l0;
                            } else {
                                self.uf_union(label, l0);
                            }
                        }
                    }
                    if label == 0 {
                        label = next;
                        next += 1;
                        self.parent[label as usize] = label;
                    }
                    cur.push((start as u32, x as u32, label));
                    all.push((label, start as u32, x as u32, y as u32));
                } else {
                    x += 1;
                }
            }
            std::mem::swap(&mut prev, &mut cur);
        }

        let mut comps: std::collections::HashMap<u32, (usize, usize, usize, usize)> =
            std::collections::HashMap::new();
        for &(label, x0, x1, y) in all.iter() {
            let root = self.uf_find(label);
            let e = comps
                .entry(root)
                .or_insert((x0 as usize, y as usize, x1 as usize, y as usize + 1));
            e.0 = e.0.min(x0 as usize);
            e.1 = e.1.min(y as usize);
            e.2 = e.2.max(x1 as usize);
            e.3 = e.3.max(y as usize + 1);
        }

        padd(P_CC, _t);
        _t = pnow();
        let min_h = (cfg.font_px * 0.3).max(2.0) as usize;
        let max_h = (cfg.font_px * cfg.max_line_h) as usize;
        let min_w = (cfg.font_px * 0.15).round().max(2.0) as usize;
        let m = cfg.margin_px.round().max(0.0) as usize;
        let dbg = std::env::var_os("M2_STATS").is_some();
        let comps_len = comps.len();
        let (mut r_h, mut r_area, mut r_dens, mut r_gap, mut r_stab, mut r_tight, mut r_ok) = (0, 0, 0, 0, 0, 0, 0);
        let mut out = Vec::new();
        for (_, (cx0, cy0, cx1, cy1)) in comps {
            // Satirlari say: bos ya da seyrek satirlar bolucudur. Yazi, arka plan dokusuna
            // baglandiginda bilesen dev olur; seyrek satirlardan bolerek yazi satirini
            // ayiriyoruz (onceki surum max_h'i asan grubu TAMAMEN reddediyordu ->
            // karakter basi isim etiketleri hic yakalanmiyordu).
            let ch = cy1 - cy0;
            let mut rowcnt = vec![0u32; ch];
            for (i, yy) in (cy0..cy1).enumerate() {
                let mut n = 0u32;
                for x in cx0..cx1 {
                    let k = yy * w + x;
                    if self.con[k] == 1 || self.pal[k] == 1 {
                        n += 1;
                    }
                }
                rowcnt[i] = n;
            }
            let peak = rowcnt.iter().copied().max().unwrap_or(0).max(1);
            let cut = ((peak as f32 * 0.30) as u32).max(1);
            // 1) dogal satir gruplari: BOS satirlar bolucudur
            let mut groups: Vec<(usize, usize)> = Vec::new();
            let mut start: Option<usize> = None;
            for (i, &n) in rowcnt.iter().enumerate() {
                if n > 0 {
                    if start.is_none() {
                        start = Some(i);
                    }
                } else if let Some(s) = start.take() {
                    groups.push((cy0 + s, cy0 + i));
                }
            }
            if let Some(s) = start {
                groups.push((cy0 + s, cy1));
            }
            // 2) cok uzun gruplar: seyrek satirlardan bol. Boylece arka plan dokusuna
            //    karisip devlesen bilesen icindeki yazi satiri da atilmaz (onceki surum
            //    max_h'i asan grubu TAMAMEN reddediyordu).
            for _ in 0..8 {
                let mut split_at = None;
                for (gi, &(a, b)) in groups.iter().enumerate() {
                    if b - a > max_h {
                        let lo = a + min_h;
                        let hi = b - min_h;
                        if lo < hi {
                            let mid = (lo..hi)
                                .map(|y| (y, rowcnt[y - cy0]))
                                .min_by_key(|(_, v)| *v)
                                .map(|(y, _)| y);
                            if let Some(m) = mid {
                                split_at = Some((gi, m));
                                break;
                            }
                        }
                    }
                }
                match split_at {
                    Some((gi, m)) => {
                        let (a, b) = groups[gi];
                        groups[gi] = (a, m);
                        groups.insert(gi + 1, (m, b));
                    }
                    None => break,
                }
            }
            let _ = cut;

            for (ly0, ly1) in groups {
                if ly1 - ly0 < min_h || cx1 - cx0 < min_w {
                    r_h += 1;
                    continue;
                }

                // alan + satir ici gecis: iki kanaldan daha "yazi gibi" olani
                let (mut area, mut trans, mut rows) = (0u32, 0u32, 0u32);
                for yy in ly0..ly1 {
                    let (mut pc, mut pp) = (0u8, 0u8);
                    let (mut ac, mut ap, mut tc, mut tp) = (0u32, 0u32, 0u32, 0u32);
                    for x in cx0..cx1 {
                        let i = yy * w + x;
                        let vc = self.con[i];
                        let vp = self.pal[i];
                        if vc == 1 {
                            ac += 1;
                            if pc == 0 {
                                tc += 1;
                            }
                        }
                        if vp == 1 {
                            ap += 1;
                            if pp == 0 {
                                tp += 1;
                            }
                        }
                        pc = vc;
                        pp = vp;
                    }
                    if ac + ap > 0 {
                        rows += 1;
                    }
                    area += ac.max(ap);
                    trans += if two_stat { tc.max(tp) } else if use_pal { tp } else { tc };
                }
                if area < 3 || rows == 0 {
                    r_area += 1;
                    continue;
                }
                let lh = ly1 - ly0;
                let density = area as f32 / ((cx1 - cx0) as f32 * lh as f32);
                if density < cfg.min_density || density > 0.92 {
                    r_dens += 1;
                    continue;
                }
                // satir ici bosluk: duz panelde 0, yazida >0
                if (trans as f32 - rows as f32) / rows as f32 <= 0.3 {
                    r_gap += 1;
                    continue;
                }
                // yalnizca "bu piksel son K karede ink oldu" olanlar: ince yazida kutu
                // alaninin %90'i arka plandir, onu saymazsak yazi da dusuk cikar
                if use_stab {
                    let mut acc = 0u32;
                    let mut cnt = 0u32;
                    for yy in ly0..ly1 {
                        for x in cx0..cx1 {
                            let v = self.stability[yy * w + x];
                            if v > 0 {
                                acc += v as u32;
                                cnt += 1;
                            }
                        }
                    }
                    let keff = STAB_K.min(self.seen) as f32;
                    let stabv = if cnt > 0 {
                        (acc as f32 / cnt as f32) / keff
                    } else {
                        0.0
                    };
                    if self.seen >= 4 && stabv < cfg.stable_min {
                        r_stab += 1;
                        continue;
                    }
                }
                r_ok += 1;

                // birlestirme genislemesini geri al: kutu yazi cekirdeginin etrafina kenetlenir
                let (mut x0, mut x1, mut y0, mut y1) = (cx0, cx1, ly0, ly1);
                while x0 < x1 && !self.edge_has(x0, y0, y1, true) {
                    x0 += 1;
                }
                while x1 > x0 && !self.edge_has(x1 - 1, y0, y1, true) {
                    x1 -= 1;
                }
                while y0 < y1 && !self.edge_has(y0, x0, x1, false) {
                    y0 += 1;
                }
                while y1 > y0 && !self.edge_has(y1 - 1, x0, x1, false) {
                    y1 -= 1;
                }
                if x1 <= x0 || y1 <= y0 {
                    r_tight += 1;
                    continue;
                }
                let (mut bx0, mut by0, mut bx1, mut by1) = (
                    x0.saturating_sub(m),
                    y0.saturating_sub(m),
                    (x1 + m).min(w),
                    (y1 + m).min(h),
                );
                if let Some(r) = cfg.roi {
                    bx0 = bx0.max(r.0);
                    by0 = by0.max(r.1);
                    bx1 = bx1.min(r.2);
                    by1 = by1.min(r.3);
                }
                if bx1 <= bx0 || by1 <= by0 {
                    continue;
                }
                out.push((bx0, by0, bx1, by1));
            }
        }
        padd(P_LINES, _t);
        if dbg && self.seen % 30 == 1 {
            eprintln!(
                "bilesen={} satir_dene={} | red: h={r_h} alan={r_area} yogunluk={r_dens} bosluk={r_gap} kararli={r_stab} kutu={r_tight} | KABUL={r_ok} cikti={} font={:.1} min_h={} max_h={} min_w={}",
                comps_len, r_h + r_area + r_dens + r_gap + r_stab + r_tight + r_ok,
                out.len(), cfg.font_px, min_h, max_h, min_w
            );
        }
        out
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    const FONT: [(&str, [&str; 5]); 6] = [
        ("M", ["101", "111", "111", "101", "101"]),
        ("E", ["111", "100", "110", "100", "111"]),
        ("T", ["111", "010", "010", "010", "010"]),
        ("I", ["111", "010", "010", "010", "111"]),
        ("N", ["101", "111", "111", "111", "101"]),
        ("2", ["111", "001", "111", "100", "111"]),
    ];

    fn glyph(c: char) -> [&'static str; 5] {
        FONT.iter().find(|(g, _)| g.chars().next() == Some(c)).unwrap().1
    }

    fn put(fb: &mut [u8], w: usize, h: usize, x: i32, y: i32, col: [u8; 3]) {
        for dy in 0..1i32 {
            for dx in 0..1i32 {
                let (px, py) = (x + dx, y + dy);
                if px >= 0 && py >= 0 && (px as usize) < w && (py as usize) < h {
                    let o = (py as usize * w + px as usize) * 3;
                    fb[o] = col[0];
                    fb[o + 1] = col[1];
                    fb[o + 2] = col[2];
                }
            }
        }
    }

    fn draw_text(fb: &mut [u8], w: usize, h: usize, x: i32, y: i32, s: &str, col: [u8; 3], sc: i32) {
        let mut cx = x;
        for ch in s.chars() {
            if ch == ' ' {
                cx += 4 * sc;
                continue;
            }
            let g = glyph(ch.to_ascii_uppercase());
            for (row, bits) in g.iter().enumerate() {
                for (b, bit) in bits.chars().enumerate() {
                    if bit == '1' {
                        for sy in 0..sc {
                            for sx in 0..sc {
                                put(fb, w, h, cx + b as i32 * sc + sx, y + row as i32 * sc + sy, col);
                            }
                        }
                    }
                }
            }
            cx += 4 * sc;
        }
    }

    fn synth(w: usize, h: usize) -> (Vec<u8>, Vec<(i32, i32)>, Vec<(i32, i32)>) {
        let mut fb = vec![0u8; w * h * 3];
        for y in 0..h {
            for x in 0..w {
                let o = (y * w + x) * 3;
                let v = (28 + ((x * 7 + y * 13) % 40) as u8) as u8;
                fb[o] = v;
                fb[o + 1] = (v as u16 * 9 / 10) as u8;
                fb[o + 2] = (v as u16 * 8 / 10) as u8;
            }
        }
        // buyuk aydinlik panel (GUI arkaplani gibi) ve parlak gok halkasi
        for y in 90..150 {
            for x in 210..300 {
                let o = (y * w + x) * 3;
                fb[o] = 235;
                fb[o + 1] = 232;
                fb[o + 2] = 225;
            }
        }
        for y in 8..50 {
            for x in 252..294 {
                let d = ((x - 273) * (x - 273) + (y - 28) * (y - 28)) as f32;
                if d < 300.0 {
                    let o = (y * w + x) * 3;
                    fb[o] = 250;
                    fb[o + 1] = 250;
                    fb[o + 2] = 250;
                }
            }
        }

        let mut marks = Vec::new();
        // siyah outline (SetOutline(true)) once arkaya, sonra altin yazi
        for y in 28..42 {
            for x in 18..70 {
                let o = (y * w + x) * 3;
                fb[o] = 0;
                fb[o + 1] = 0;
                fb[o + 2] = 0;
            }
        }
        draw_text(&mut fb, w, h, 20, 30, "METIN2", [255, 204, 0], 2);
        for y in 30..40 {
            for x in 20..68 {
                marks.push((x as i32, y as i32));
            }
        }
        let white: Vec<(i32, i32)> = (120..125).flat_map(|y| (60..70).map(move |x| (x, y))).collect();
        draw_text(&mut fb, w, h, 60, 120, "MT2", [255, 255, 255], 1);
        (fb, marks, white)
    }

    fn hit_ratio(boxes: &[Rect], pts: &[(i32, i32)]) -> f32 {
        if pts.is_empty() {
            return 1.0;
        }
        let n = pts
            .iter()
            .filter(|(x, y)| {
                boxes.iter().any(|r| {
                    *x >= r.0 as i32 && *x < r.2 as i32 && *y >= r.1 as i32 && *y < r.3 as i32
                })
            })
            .count();
        n as f32 / pts.len() as f32
    }

    #[test]
    #[ignore]
    fn hiz_720p() {
        let (w, h) = (1280, 720);
        let mut fb = vec![0u8; w * h * 3];
        for y in 0..h {
            for x in 0..w {
                let o = (y * w + x) * 3;
                let v = 40 + ((x * 7 + y * 13) % 60) as u8;
                fb[o] = v;
                fb[o + 1] = v;
                fb[o + 2] = v;
            }
        }
        for i in 0..6 {
            draw_text(&mut fb, w, h, 40, 40 + i * 30, "METIN2", [255, 204, 0], 1);
        }
        let cfg = Cfg::default();
        let mut d = Detector::new(w, h);
        let t = std::time::Instant::now();
        let mut boxes = 0;
        for _ in 0..10 {
            boxes = d.detect(&fb, &cfg).len();
        }
        let ms = t.elapsed().as_millis() as f32 / 10.0;
        println!("720p: {ms:.1} ms/kare ({:.1} fps), {boxes} kutu", 1000.0 / ms);
        assert!(ms < 400.0, "algilama cok yavas: {ms:.1} ms");
    }

    #[test]
    fn metin2_yazisi_yakinlanir_arka_plan_yok() {
        let (w, h) = (320, 180);
        let (frame, gold, white) = synth(w, h);
        let cfg = Cfg {
            font_px: 14.0,
            merge_r: 6,
            ..Default::default()
        };
        let mut d = Detector::new(w, h);
        let boxes = d.detect(&frame, &cfg);
        assert!(!boxes.is_empty(), "hic kutu bulunamadi");

        let gr = hit_ratio(&boxes, &gold);
        assert!(gr > 0.9, "altin isim yazisi kapsanmadi ({gr:.2}): {boxes:?}");

        let wr = hit_ratio(&boxes, &white);
        assert!(wr > 0.9, "beyaz seviye yazisi kapsanmadi ({wr:.2}): {boxes:?}");

        let panel: Vec<(i32, i32)> = (95..145)
            .flat_map(|y| (215..295).map(move |x| (x, y)))
            .collect();
        let pr = hit_ratio(&boxes, &panel);
        assert!(pr < 0.1, "GUI paneli yanlis pozitif ({pr:.2}): {boxes:?}");
    }
}

// ---- gecici profil (M2_TIME=1): asama sureleri ----
pub const P_SPLIT: usize = 0;
pub const P_MORPH: usize = 1;
pub const P_MASK: usize = 2;
pub const P_STAB: usize = 3;
pub const P_MERGE: usize = 4;
pub const P_CC: usize = 5;
pub const P_LINES: usize = 6;
pub const P_BLUR: usize = 7;
pub const P_N: usize = 8;
pub static PROF: [std::sync::atomic::AtomicU64; P_N] = [const { std::sync::atomic::AtomicU64::new(0) }; P_N];
pub static PROF_N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

#[inline]
pub fn pnow() -> std::time::Instant {
    std::time::Instant::now()
}
#[inline]
pub fn padd(i: usize, t0: std::time::Instant) {
    use std::sync::atomic::Ordering::Relaxed;
    PROF[i].fetch_add(t0.elapsed().as_nanos() as u64, Relaxed);
}
pub fn prof_dump() {
    use std::sync::atomic::Ordering::Relaxed;
    let n = PROF_N.load(Relaxed).max(1) as f64;
    let names = ["kanal_ayir", "morfoloji", "maske", "kararlilik", "yatay_birlestir", "bilesen", "satirlar", "bulanik"];
    let mut tot = 0.0;
    print!("\n--- asama suresi (kare basina ms) ---\n");
    for i in 0..P_N {
        let ms = PROF[i].load(Relaxed) as f64 / n / 1e6;
        tot += ms;
        println!("  {:<18} {:6.2} ms", names[i], ms);
    }
    println!("  {:<18} {:6.2} ms", "TOPLAM", tot);
}
