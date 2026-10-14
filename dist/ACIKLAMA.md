# Forge Design

## Kısa açıklama

Forge Design, Rust ile yazılmış, açık kaynaklı ve hızlı bir raster görsel editörüdür. Photoshop'un iş akışını ve kısayollarını ücretsiz bir uygulamada sunar.

## Uzun açıklama

Forge Design, Adobe Photoshop'a alternatif olarak tasarlanmış, açık kaynaklı bir görsel düzenleme uygulamasıdır. Motorun tamamı Rust ile yazıldığı için büyük görsellerde bile hızlı çalışır: piksel verisi 64×64'lük döşemeler halinde ve kopyala-yaz (copy-on-write) mantığıyla tutulur, filtreler ve kompozit işlemleri tüm işlemci çekirdeklerinde paralel çalışır.

**Neler yapabilirsiniz**

- **Katmanlar:** katman, grup, kırpma maskesi, katman maskesi, dolgu opaklığı, renk etiketleri, 27 Photoshop karıştırma modu ve tam katman stilleri (gölge, parıltı, kabartma, kontur, degrade ve desen kaplamaları).
- **Seçim ve maskeleme:** dikdörtgen, elips, lasso, çokgen lasso, manyetik lasso, hızlı seçim, sihirli değnek, renk aralığı, odak alanı, kanal ve yol (path) tabanlı seçimler.
- **Çizim ve retuş:** fırça (uç açısı, yuvarlaklık, saçılma, simetri, özel uçlar), kalem, klon ve desen damgası, iyileştirme fırçası, yama, kırmızı göz, yakma/parlatma/süngerleme, bulanıklaştırma/keskinleştirme, parmak boyama, sıvılaştırma (Liquify) ve içeriğe duyarlı dolgu.
- **Ayarlamalar ve filtreler:** Levels, Curves, Hue/Saturation, Color Balance, Channel Mixer, Selective Color, 3D LUT (.cube) ve onlarca filtre. Ayarlamalar canlı önizlemeli, katman olarak da uygulanabilir.
- **Dönüşüm:** serbest dönüştürme, eğme, bozma, perspektif, ağ (warp) ve perspektif kırpma.
- **Metin ve şekiller:** düzenlenebilir metin katmanları (kalın/italik, izleme, satır aralığı, kutu içinde kaydırma, yay), 150 yerleşik font ve kendi .ttf/.otf fontlarınızı ekleme veya sürükle-bırak ile kurma. Dikdörtgen, elips, çokgen, yıldız, ok, kalp ve konuşma balonu gibi şekiller.
- **Yollar (Pen):** Bézier yolları, yoldan seçim veya dolgu, SVG dışa aktarma.
- **Dosya biçimleri:** kendi katmanlı formatı OpenRaster (.ora), Photoshop PSD (okuma ve yazma, grup ve maskeler dahil), PNG, JPEG, WebP, TIFF, BMP, GIF, TGA, QOI, PDF, SVG; animasyonlu GIF ve katmanları ayrı dosyalara aktarma.
- **Çalışma akışı:** Photoshop kısayolları, cetveller, kılavuzlar, ızgara, yakalama (snap), kanıtlama (proof) renkleri, hızlı maske, Actions (kaydet ve oynat), toplu işlem (batch), otomatik kaydetme ve kurtarma.
- **Arayüz:** Projects başlangıç ekranı ve son projeler listesi, tek sütunlu araç çubuğu, sekmeli paneller (Color, Properties, Layers, History ve diğerleri), açık/koyu tema.
- **Komut satırı:** `forge-cli` ile toplu işleme (yeniden boyutlandırma, keskinleştirme, dışa aktarma).

**Lisans ve platform:** Apache License 2.0 ile açık kaynak. Windows için kurulum sihirbazı, Linux ve macOS'ta kaynaktan derlenebilir.

Made by burkiuze.
