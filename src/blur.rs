// AyriIabilir dikdortgen cekirdekli gri morfoloji (once satir, sonra sutun).
fn slide_taps(p: &[u8], dst: &mut [u8], len: usize, r: usize, want_max: bool) {
    let mut m = p[r];
    if want_max {
        for k in 1..=2 * r {
            m = m.max(p[r + k]);
        }
    } else {
        for k in 1..=2 * r {
            m = m.min(p[r + k]);
        }
    }
    dst[0] = m;
    for i in 1..len {
        let m = if want_max {
            p[i].max(p[i + 2 * r])
        } else {
            p[i].min(p[i + 2 * r])
        };
        dst[i] = if want_max { m.max(dst[i - 1]) } else { m.min(dst[i - 1]) };
    }
}

pub fn morph_gray(src: &[u8], w: usize, h: usize, r: usize, want_max: bool) -> Vec<u8> {
    let mut tmp = vec![0u8; src.len()];
    let mut out = vec![0u8; src.len()];
    if r == 0 {
        return src.to_vec();
    }
    let mut pad = vec![0u8; w + 2 * r + 2];
    for y in 0..h {
        let row = &src[y * w..y * w + w];
        pad[r..r + w].copy_from_slice(row);
        for i in 0..r {
            pad[i] = row[0];
            pad[r + w + i] = row[w - 1];
        }
        slide_taps(&pad, &mut tmp[y * w..y * w + w], w, r, want_max);
    }
    // Dikey gecis sutun sutun yazilirsa her yazma bir satir atlaga duser (w bayt)
    // ve duzlem cache'e tekrar tekrar tasinir -> 720p'de kare basina 19 ms. 64
    // sutunluk bant tamponunda calisiyoruz: giris/cikis satir satir (cache dostu),
    // is yalnizca bant uzerinde sutun sutun.
    let bw = w.min(BAND_M);
    let mut line = vec![0u8; h + 2 * r + 2];
    let mut res = vec![0u8; h];
    let mut ib = vec![0u8; h * bw];
    let mut ob = vec![0u8; h * bw];
    let mut x0 = 0;
    while x0 < w {
        let b = (w - x0).min(bw);
        for y in 0..h {
            let s = y * w + x0;
            ib[y * b..y * b + b].copy_from_slice(&tmp[s..s + b]);
        }
        for x in 0..b {
            for y in 0..h {
                line[y + r] = ib[y * b + x];
            }
            for i in 0..r {
                line[i] = line[r];
                line[h + r + i] = line[h + r - 1];
            }
            slide_taps(&line, &mut res, h, r, want_max);
            for y in 0..h {
                ob[y * b + x] = res[y];
            }
        }
        for y in 0..h {
            let d = y * w + x0;
            out[d..d + b].copy_from_slice(&ob[y * b..y * b + b]);
        }
        x0 += b;
    }
    out
}

// Sadece yatay genisletme: ayni satirdaki sozcukleri birlestirir, altindaki satira
// tasmaz. Metin2 ekraninda iz/damage yazilari ust uste binerek dev bloga donusuyordu.
pub fn dilate_h(mask: &[u8], w: usize, h: usize, r: usize) -> Vec<u8> {
    if r == 0 {
        return mask.to_vec();
    }
    let mut out = vec![0u8; mask.len()];
    let mut pre = vec![0u32; w + 1];
    for y in 0..h {
        let row = &mask[y * w..y * w + w];
        pre[0] = 0;
        for i in 0..w {
            pre[i + 1] = pre[i] + row[i] as u32;
        }
        for i in 0..w {
            let lo = i.saturating_sub(r);
            let hi = (i + r + 1).min(w);
            out[y * w + i] = (pre[hi] > pre[lo]) as u8;
        }
    }
    out
}

// --- bulaniklastirma ---------------------------------------------------------
// 1080p olcumunde asagidaki iki sey kare basina 1.25 sn tutuyordu:
//  1) tespit 672 kutu uretiyor; her kutu radius*2 kadar sisirilip AYRI ayri
//     bulaniklaniyordu -> ust uste binen pikseller 4-6 kez isleniyordu (8.8 Mpx,
//     yani karenin 4.2 kati). Simdi kutular birlestiriliyor: kapsama alani tek
//     parcaya indiriliyor, her piksel yalnizca BIR kez bulaniklaniyor.
//  2) dikey gecis sutun sutun ( satir basina w*3 atlama) yürüyordu; simdi 64
//     sutunluk bant tamponuna toplanip orada calisiyor (cache'de kalir).
// tamponlar kareler arasi yeniden kullanilir (kutu basina yeni Vec yok).
const BAND: usize = 64;
const BAND_M: usize = 64;
const PASSES: usize = 2;

pub struct BlurCtx {
    cov: Vec<u8>,
    rects: Vec<(usize, usize, usize, usize)>,
    scr: Vec<Scratch>,
}

// Her is parcaciginin kendi tamponlari (paylasim yok = tartisma yok)
struct Scratch {
    a: Vec<u8>,
    b: Vec<u8>,
    ba: Vec<u8>,
    bb: Vec<u8>,
    pre: Vec<u32>,
    inv: Vec<f32>,
}

impl Scratch {
    fn new() -> Self {
        Scratch { a: Vec::new(), b: Vec::new(), ba: Vec::new(), bb: Vec::new(), pre: Vec::new(), inv: Vec::new() }
    }
}

impl BlurCtx {
    pub fn new(w: usize, h: usize) -> Self {
        let nth = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(2).min(4);
        BlurCtx { cov: vec![0; w * h], rects: Vec::new(), scr: (0..nth).map(|_| Scratch::new()).collect() }
    }

    pub fn apply(
        &mut self,
        buf: &mut [u8],
        w: usize,
        h: usize,
        rects: &[(usize, usize, usize, usize)],
        radius: usize,
    ) {
        if rects.is_empty() || radius == 0 {
            return;
        }
        // 1) kapsama maskesi
        if self.cov.len() != w * h {
            self.cov.resize(w * h, 0);
        }
        self.cov.fill(0);
        let pad = radius * 2;
        for &(x0, y0, x1, y1) in rects {
            let (a0, b0) = (x0.saturating_sub(pad), y0.saturating_sub(pad));
            let (a1, b1) = ((x1 + pad).min(w), (y1 + pad).min(h));
            if a1 > a0 && b1 > b0 {
                for y in b0..b1 {
                    self.cov[y * w + a0..y * w + a1].fill(1);
                }
            }
        }
        // 2) acgoya birlestir: satir satir en soldaki kapli pikseli bul, satiri
        //    tarama genisligince uzat, sonra tum genislik kapli oldugu surece asagi in
        self.rects.clear();
        for y in 0..h {
            let base = y * w;
            let mut x = 0;
            while x < w {
                if self.cov[base + x] == 0 {
                    x += 1;
                    continue;
                }
                let rx0 = x;
                while x < w && self.cov[base + x] == 1 {
                    x += 1;
                }
                let rx1 = x;
                let mut ry1 = y + 1;
                while ry1 < h {
                    let b = ry1 * w;
                    if self.cov[b + rx0..b + rx1].iter().all(|&v| v == 1) {
                        ry1 += 1;
                    } else {
                        break;
                    }
                }
                for yy in y..ry1 {
                    self.cov[yy * w + rx0..yy * w + rx1].fill(0);
                }
                self.rects.push((rx0, y, rx1, ry1));
            }
        }
        // is parcaciklarinin dengeli olmasi icin buyukten kucuge sirala
        self.rects
            .sort_unstable_by_key(|r| std::cmp::Reverse((r.2 - r.0) * (r.3 - r.1)));
        if std::env::var_os("M2_RECTS").is_some() {
            let area: usize = self.rects.iter().map(|r| (r.2 - r.0) * (r.3 - r.1)).sum();
            eprintln!(
                "birlestirilmis kutu: {} parca, {} kpx (kare {} kpx), en buyuk {}x{}",
                self.rects.len(),
                area / 1000,
                w * h / 1000,
                self.rects.iter().map(|r| r.2 - r.0).max().unwrap_or(0),
                self.rects.iter().map(|r| r.3 - r.1).max().unwrap_or(0)
            );
        }
        // 3) Her parca icin 2 gecis ayrilabilir kutu bulanikligi. Parcalar birbirine
        //    DEGMEZ (acgoya birlestirme tekrar sayma garantisi veriyor), bu yuzden is
        //    parcaciklari farkli parcalari guvenle isleyebilir.
        //    NOT: uzun parcalari satir seritlerine bolmeyi denedim -> is parcaciklari
        //    daha dengeli olsa da radius kadar ustuste binen seritler yuzunden
        //    18 -> 20 ms, yani daha YAVAS. Basit yol birakildi.
        let base = buf.as_mut_ptr() as usize;
        let rects = &self.rects[..];
        let n = self.scr.len().min(self.rects.len().max(1));
        let mut scr = std::mem::take(&mut self.scr);
        std::thread::scope(|sc| {
            for (k, sth) in scr.iter_mut().enumerate() {
                sc.spawn(move || {
                    let mut i = k;
                    while i < rects.len() {
                        let rc = rects[i];
                        // SAFETY: acgoya birlestirilen dikdortgenler birbirine degmez,
                        // yazdigimiz baytlar tamamen bu parcaciga ait
                        unsafe {
                            let sl = std::slice::from_raw_parts_mut(
                                (base + ((rc.1 * w + rc.0) * 3)) as *mut u8,
                                (rc.3 - rc.1) * w * 3,
                            );
                            blur_rect(sl, w, rc.2 - rc.0, rc.3 - rc.1, radius, sth);
                        }
                        i += n;
                    }
                });
            }
        });
        self.scr = scr;
    }
}

// sl: yalnizca dikdortgenin satirlarini icerir; w tam kare genisligidir.
fn blur_rect(sl: &mut [u8], w: usize, rw: usize, rh: usize, radius: usize, sc: &mut Scratch) {
    if rw == 0 || rh == 0 {
        return;
    }
    let Scratch { a, b, ba, bb, pre, inv } = sc;
    let need = rw * rh * 3;
    if a.len() < need {
        a.resize(need, 0);
        b.resize(need, 0);
    }
    for y in 0..rh {
        let so = y * w * 3;
        let dof = y * rw * 3;
        a[dof..dof + rw * 3].copy_from_slice(&sl[so..so + rw * 3]);
    }
    for _ in 0..PASSES {
        box_h(a, b, rw, rh, radius, pre, inv);
        box_v(b, a, rw, rh, radius, ba, bb, pre, inv);
    }
    for y in 0..rh {
        let so = y * w * 3;
        let dof = y * rw * 3;
        sl[so..so + rw * 3].copy_from_slice(&a[dof..dof + rw * 3]);
    }
}

// Pencere uzunlugu -> 1/uzunluk. Piksel basina bolme yerine carpma.
fn recip(max: usize, inv: &mut Vec<f32>) {
    if inv.len() <= max {
        inv.resize(max + 1, 0.0);
        for (i, v) in inv.iter_mut().enumerate() {
            *v = 1.0 / i.max(1) as f32;
        }
    }
}

// Yatay kutu: on eklemeli toplam, O(1)/piksel, satir satir (cache dostu).
fn box_h(
    src: &[u8],
    dst: &mut [u8],
    w: usize,
    h: usize,
    r: usize,
    pre: &mut Vec<u32>,
    inv: &mut Vec<f32>,
) {
    if pre.len() < w + 1 {
        pre.resize(w + 1, 0);
    }
    recip(2 * r + 1, inv);
    let inv = &*inv;
    for y in 0..h {
        let row = y * w * 3;
        for c in 0..3 {
            pre[0] = 0;
            for i in 0..w {
                pre[i + 1] = pre[i] + src[row + i * 3 + c] as u32;
            }
            for i in 0..w {
                let lo = i.saturating_sub(r);
                let hi = (i + r + 1).min(w);
                dst[row + i * 3 + c] = ((pre[hi] - pre[lo]) as f32 * inv[hi - lo]) as u8;
            }
        }
    }
}

// Dikey kutu: 64 sutunluk bant halinde toplanip orada on eklemeli toplam. Tam
// karede dogrudan yapsaydik her yazma satiri w*3 bayt atlayardi (cache isabeti
// dusuk, 5-8 kat yavas).
fn box_v(
    src: &[u8],
    dst: &mut [u8],
    w: usize,
    h: usize,
    r: usize,
    ba: &mut Vec<u8>,
    bb: &mut Vec<u8>,
    pre: &mut Vec<u32>,
    inv: &mut Vec<f32>,
) {
    let bw = w.min(BAND);
    recip(2 * r + 1, inv);
    if ba.len() < 3 * h * bw {
        ba.resize(3 * h * bw, 0);
        bb.resize(3 * h * bw, 0);
    }
    if pre.len() < h + 1 {
        pre.resize(h + 1, 0);
    }
    let (ba, bb) = (&mut *ba, &mut *bb);
    let inv = &*inv;
    let mut x0 = 0;
    while x0 < w {
        let b = (w - x0).min(bw);
        let (p1, p2) = (h * b, 2 * h * b);
        for y in 0..h {
            let s = (y * w + x0) * 3;
            let d = y * b;
            for x in 0..b {
                ba[d + x] = src[s + x * 3];
                ba[p1 + d + x] = src[s + x * 3 + 1];
                ba[p2 + d + x] = src[s + x * 3 + 2];
            }
        }
        for (c, base) in [0usize, p1, p2].into_iter().enumerate() {
            let _ = c;
            for x in 0..b {
                pre[0] = 0;
                for y in 0..h {
                    pre[y + 1] = pre[y] + ba[base + y * b + x] as u32;
                }
                for y in 0..h {
                    let lo = y.saturating_sub(r);
                    let hi = (y + r + 1).min(h);
                    bb[base + y * b + x] = ((pre[hi] - pre[lo]) as f32 * inv[hi - lo]) as u8;
                }
            }
        }
        for y in 0..h {
            let s = (y * w + x0) * 3;
            let d = y * b;
            for x in 0..b {
                dst[s + x * 3] = bb[d + x];
                dst[s + x * 3 + 1] = bb[p1 + d + x];
                dst[s + x * 3 + 2] = bb[p2 + d + x];
            }
        }
        x0 += b;
    }
}
