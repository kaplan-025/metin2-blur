// KENARLAR ikilenmis tampon uzerinde sabit tap min/max. r kucuk (1-3) oldugu icin
// monoton dequeden hizli ve kodu daha kisadir.
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

// AyriIabilir dikdortgen cekirdekli gri morfoloji (once satir, sonra sutun).
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
    let mut line = vec![0u8; h + 2 * r + 2];
    let mut res = vec![0u8; h];
    for x in 0..w {
        for y in 0..h {
            line[y + r] = tmp[y * w + x];
        }
        for i in 0..r {
            line[i] = line[r];
            line[h + r + i] = line[h + r - 1];
        }
        slide_taps(&line, &mut res, h, r, want_max);
        for y in 0..h {
            out[y * w + x] = res[y];
        }
    }
    out
}

// window uzunlugu -> 1/uzunluk. Bolme yerine carpma (piksel basina tam sayi bolmesi pahaliydi).
fn recip(max: usize) -> Vec<f32> {
    (0..=max).map(|i| 1.0 / i.max(1) as f32).collect()
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

// Ayrilabilir kutu (box) bulanikligi: 3 gecis bir Gauss'a yakin sonuc verir.
pub fn blur_region(buf: &mut [u8], w: usize, _h: usize, rect: (usize, usize, usize, usize), radius: usize) {
    let (x0, y0, x1, y1) = rect;
    if x1 <= x0 || y1 <= y0 || radius == 0 {
        return;
    }
    let rw = x1 - x0;
    let rh = y1 - y0;
    let mut src = vec![0u8; rw * rh * 3];
    for y in 0..rh {
        let so = ((y + y0) * w + x0) * 3;
        let dof = y * rw * 3;
        src[dof..dof + rw * 3].copy_from_slice(&buf[so..so + rw * 3]);
    }

    let mut tmp = vec![0u8; rw * rh * 3];
    for _ in 0..3 {
        box_h(&src, &mut tmp, rw, rh, radius);
        box_v(&tmp, &mut src, rw, rh, radius);
    }

    for y in 0..rh {
        let dof = ((y + y0) * w + x0) * 3;
        buf[dof..dof + rw * 3].copy_from_slice(&src[y * rw * 3..y * rw * 3 + rw * 3]);
    }
}

fn box_h(src: &[u8], dst: &mut [u8], w: usize, h: usize, r: usize) {
    let inv = recip(2 * r + 1);
    let mut pre = vec![0u32; w + 1];
    for y in 0..h {
        let row = &src[y * w * 3..y * w * 3 + w * 3];
        for c in 0..3 {
            pre[0] = 0;
            for i in 0..w {
                pre[i + 1] = pre[i] + row[i * 3 + c] as u32;
            }
            let out = &mut dst[y * w * 3..y * w * 3 + w * 3];
            for i in 0..w {
                let lo = i.saturating_sub(r);
                let hi = (i + r + 1).min(w);
                out[i * 3 + c] = ((pre[hi] - pre[lo]) as f32 * inv[hi - lo]) as u8;
            }
        }
    }
}

fn box_v(src: &[u8], dst: &mut [u8], w: usize, h: usize, r: usize) {
    let inv = recip(2 * r + 1);
    let mut col = vec![0u8; h];
    let mut pre = vec![0u32; h + 1];
    for x in 0..w {
        for c in 0..3 {
            for y in 0..h {
                col[y] = src[(y * w + x) * 3 + c];
            }
            pre[0] = 0;
            for y in 0..h {
                pre[y + 1] = pre[y] + col[y] as u32;
            }
            for y in 0..h {
                let lo = y.saturating_sub(r);
                let hi = (y + r + 1).min(h);
                dst[(y * w + x) * 3 + c] = ((pre[hi] - pre[lo]) as f32 * inv[hi - lo]) as u8;
            }
        }
    }
}
