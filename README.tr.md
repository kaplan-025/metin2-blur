<div align="center">

# Metin2 Blur

**Yerel, çevrimdışı video metin redaksiyon aracı — Metin2 kayıtları için ayarlanmış.**
Tek dosya Windows çalıştırılabilir. Hiçbir şey hiçbir yere yüklenmez.

[English](README.md) · [Türkçe](README.tr.md)

</div>

![önce / sonra](docs/demo.png)

*Sol: 640×480 Metin2 kaydı. Sağ: aynı karenin redakte edilmiş hâli — isim etiketleri,
uçan hasar sayıları, sohbet ve HUD yazıları bulanık. [Tam demo videosu →](https://github.com/kaplan-025/metin2-blur/releases/download/v1.0.0/demo-redacted.mp4)*

![Arayüz](docs/ui.png)

*Önizlemede çizilen bölgeler (turuncu) koşulsuz bulanıklaştırılır; yeşil kutular o karede otomatik bulunanlardır.*

---

## Ne yapar

Bir video dosyası alır, ekrandaki **yazıyı** bulur ve o bölgeleri bulanıklaştırır.
Oyun kayıtlarını paylaşmadan önce isimleri, sohbeti ve hesap kimliklerini
gizlemek için kullanılır: hata raporu, oyun destek talebi (ticket), VOD ya da
yanındakilerin ve kendi hesabının adının görünmesini istemediğin her durum.

Bu bir **redaksiyon yardımcısıdır, garanti değildir.** Hassas bir şey için
kullanmadan önce [Sınırlar](#sınırlar) bölümünü oku ve çıktıyı kendin kontrol et.

## Metin2 kayıtlarında neden çalışıyor

Tespit OCR değil — karakter tanıma yok, bulut yok, model indirme yok. Sızan
client kaynaklarından Metin2'nin yazıyı çok belirli bir şekilde çizdiği görülüyor:

* `EterLib/GrpFontTexture.cpp` — glifler `ANTIALIASED_QUALITY` ile çizilip
  **1-bit maskeye eşikleniyor**, `A4R4G4B4` olarak yükleniyor ve çizim anında
  renkle modulate ediliyor. Ekranda bu, doku değil, **düz renk + yumuşak kenar**
  demek.
* `EterLib/GrpText.cpp` — font kaynakları `isim:12.fnt`, `14i.fnt` (italik),
  `18.fnt`; varsayılan **12 px**.
* `UserInterface/PythonTextTail.cpp` — isim etiketlerinde `SetOutline(true)`
  (siyah outline) ve yazının arkasında koyu kutu; renkler sabit liste (oyuncu
  beyaz, canavar kırmızı, guild `0xFFEFD4FF`, guild seviyeleri `CD661D`/`FFC125`,
  seviye renkleri `00FF7F`…`FF4500` …).

Bunun sonucu olan boru hattı:

```
kare → gri tonlamalı açma (erosion→dilation) → top-hat / black-hat  ← ince yapı tepkisi
     ∪ Metin2 palet çekirdeği (5-bit LUT)                          ← renkten gelen hatırlatma
     → yatay run birleştirme → bağlı bileşenler → satıra bölme
     → boyut / doluluk / satır içi boşluk / zaman-kararlılık filtreleri
     → bölge başına kutu bulanıklığı → ffmpeg ile yeniden kodlama
```

Kil işlem **gri tonlamalı açma**: sıradan yerel kontrast ölçümünün aksine kalın
parlak alanları (gökyüzü, arayüz panelleri) siler, glif çizgileri arasındaki
**boşlukları korur**; video sıkıştırması ve büyütmede sonra tutmasının sebebi bu.

> Palet eşleştirmesi **tek başına yetmez**: `yuv420p` kroma alt örneklemesi ve
> H.264 düz renkleri kaydırıyor, `(255,204,0)` altın ölçülebilir biçimde
> referanstan uzaklaşıyor. Bu yüzden tespiti yapı yapı taşıyor, palet sadece
> hatırlatma ekliyor.

## Dikey kayitlar (720x1280 telefon kaydi)

![dikey kayit](docs/demo-vertical.png)

Telefon/portre kayitlarinda yazilar 10-12 px'e dusable. Karakter adi, item
tooltip, Yang sayaci ve sohbet olcumde tamamen kapatildi; karakter basi isim
etiketinin bir kismi kalmaya devam edebiliyor (bkz. [Sinirlar](#sinirlar)).

## Kurulum

**Windows** — releases sayfasından `Metin2Blur.exe` indir. Kurulum tamamen bu:
ffmpeg yok, Python yok, ek çalıştırma kitaplığı yok. `ffmpeg.exe` binary'nin
içine gömülü ve ilk çalıştırmada `%LOCALAPPDATA%\metin2-blur\` altına açılıyor.
(İmzasız, yeni indirilen dosyada Windows SmartScreen uyarırsa
*Daha fazla bilgi → Yine de çalıştır*.)

**Linux** — `PATH` üzerinde ffmpeg gerekir:

```bash
cargo build --release
./target/release/metin2-blur
```

## Kullanım

1. Videoyu pencereye bırak ya da yolu yapıştırıp **Yükle**.
2. **Önizle** — bir kare çözülür, bulunan metin maskesi kırmızı, kutular yeşil
   çizilir. Önemli olan adım bu: umursadığın bir isim kutu içinde değilse
   üstüne bölge çiz.
3. İstersen **İşlem alanı (ROI)**'yi oyun penceresine ayarla; böylece duvar
   kağıdı, masaüstü araçları ve diğer pencereler tamamen devre dışı kalır.
4. **Başlat**. İlerleme çubuğu `kare n/toplam`, hız, bulunan bölge sayısı ve
   tahmini kalan süreyi gösterir. **İptal** durdurur.

### Bölgeler: en güvenilir yol

Metin2 arayüzü **sabit konumda** olduğu için, düz bir dikdörtgen çoğu zaman
otomatik tespitten daha isabetlidir — üstelik yanlış pozitifi olmaz:

* **Önizlemede bölge çiz** kutusunu aç, sohbetin / HUD'ın / envanterin
  üzerinden sürükle; en fazla 4 bölge.
* Her bölge her karede **koşulsuz** bulanıklaştırılır.

### Komut satırı

```
Metin2Blur.exe --cli <video> [cikti] [hassasiyet] [font_px] [roi] [mod] [bolgeler] [tespit_1/N]

  hassasiyet  0 = katı, 1 = her şeyi yakala            (varsayılan 0.55)
  font_px      videodaki beklenen yazı yüksekliği, px   (varsayılan: 12 × yükseklik/768)
  roi          yüzde x,y,g,y                           (0,0,100,100 = tüm ekran)
  mod          ikisi | palet | yapi
  bolgeler     yüzde x,y,g,y;x,y,g,y — hep bulanıklaştırılır
  tespit_1/N   tespit kaç karede bir (1 = her kare, 2-3 = hızlı)

örnek:
  Metin2Blur.exe --cli oyun.mp4 cikti.mp4 0.8 12 0,0,100,100 ikisi 0,46,55,30;16,7,20,5
```

`FFMPEG=/path/to/ffmpeg` gömülü binary'yi geçersiz kılar.

## Sınırlar

Tahmin değil, ölçüm — bu bölümü lütfen oku:

* **Otomatik tespit %100 değil — ölçülmüş.** Metin2'de iki sert durum var:
  * *Karakter başı isim etiketi, parlak/dokulu 3B sahnenin üzerinde:* dikey
    720×1280 kayıtta etiketin sağ yarısı bulanıklaşıyor, sol yarısı %31
    okunabilirlikte kalabiliyor. Sebep: koyu kırmızı yazının (81,35,25) parlak
    çim üzerindeki yerel tepkisi (p90 = 59) eşik sınırında ve dokuyla iç içe.
  * *Masaüstü kaydı, küçük oyun penceresi:* 8 px yazı ile 3B dokunun kontrast
    dağılımları örtüştüğü için bazı uçan etiketler geçebiliyor.
  → Bu iki durumda **bölge çiz**; oyun düzeni sabit olduğu için en isabetli yöntem
  o. v1.0.1'de ölçülen iyileşme (720×1280): karakter adı %4.5→%0.0, item
  tooltip %1.8→%0.2, Yang sayacı %9.6→%0.0, sohbet %0→%0.
* Tam ekran kaydında çok daha güçlü (test: 7/7 metin bölgesi orijinal
  keskinliğin 0.01–0.04'üne indi, arayüz panelinde yanlış pozitif yok).
* **Okunamaması kesin olması gereken her şey için bölge kullan.** Sabit oyun
  düzeni tam bunun için var.
* Başarısızlık yönü aşırı bulanıklaştırmak: dokulu sahnelerde 3B manzarayı
  lekeleyebilir. Rahatsız ediyorsa hassasiyeti düşür ya da ROI ile sınırla.
* `font_px`'in ~3,2 katından uzun yazı arayüz paneli sayılıp atlanır.
* Palet toleransının pratik tabanı 12'dir (5-bit LUT).

## Hız

Uçtan uca ölçüldü (release, AMD Ryzen 5 1600, 4 çekirdek, ffmpeg çözme +
kodlama dahil). "1/N" = tespit her N karede bir çalışıyor, ara kareler son
kutuları kullanıyor (`--cli ... 1/N` ya da arayüzdeki *Tespit kaç karede bir*
kaydırıcısı).

| Video | 1/1 | 1/2 | 1/3 |
|---|---|---|---|
| 640x480, 30 fps, 45.9 sn | **23.2 sn** (2.0× gerçek zaman) | 18.1 sn (2.5×) | 16.3 sn (2.8×) |
| 720x1280, 30 fps, 6.6 sn | 8.8 sn (0.75×) | **6.6 sn** (1.0×) | 6.0 sn (1.1×) |
| 1920x1080, 30 fps, 6.6 sn | 17.7 sn (0.37×) | 13.3 sn (0.49×) | 11.9 sn (0.55×) |

720x1280'de kare başına: 20 ms morfoloji (3 iş parçacığı), 17 ms bulanık (4
iş parçacığı, birleştirilmiş kutular), 5 ms gerisi. v1.0.1 aynı video için
620 ms/kare gerektiriyordu — bu sürüm **11–16× daha hızlı** ve redaksiyon
birebir aynı (ölçülen: isim etiketi %31.1, tooltip %0.4, sohbet %0.0, Yang
sayacı %0.3).

Gerçek zaman için 720p'de kare başına ~33 ms gerekiyor: 640x480 rahatça gerçek
zamanın üstünde, 720x1280 1/2 veya 1/3 modda erişiyor. 1080p 4 çekirdekte
gerçek zamana ULAŞMIYOR — kaydı çözünürlüğü küçült ya da bölge kullan (bölge
tespit hızından bağımsız olarak her karede bulanıklanır).

## Kaynaktan derleme

```bash
cargo test --release          # dedektör birim testi + 720p ölçümü
cargo build --release
./build-windows.sh            # rustup target + mingw-w64 + ffmpeg-win.exe gerekir
```

`ffmpeg-win.exe` repoda yok (82 MB); release iş akışı otomatik indirir.

## Lisans

MIT — [LICENSE](LICENSE).
