package com.kokannadian.lillith.voice;

import java.util.ArrayDeque;
import java.util.Arrays;
import java.util.List;

/**
 * Port of tools/lillith-voice/lillith_voice/dsp.py: the "polish" chain for
 * Lillith's cloned voice (trim -> HPF/EQ + STFT de-esser -> loudness match
 * -> look-ahead limiter -> fades), plus crossfade_concat / fade / gap. Same
 * constants as the desktop "lillith" preset. Pure Java, double precision.
 */
public final class Dsp {
    private Dsp() {}

    public static final double LUFS = -17.0, TRUE_PEAK = -1.5, HIGHPASS = 90.0, DEESS_DB = 6.0;
    private static final double[][] BELLS = {{250, 1.0, -1.5}, {3000, 0.9, 1.5}};
    private static final double[] HISHELF = {9000, -3.0};

    // ------------------------------------------------------------ helpers

    static double[] linspace(double a, double b, int n) {
        double[] r = new double[n];
        if (n == 1) {
            r[0] = a;
            return r;
        }
        double step = (b - a) / (n - 1);
        for (int i = 0; i < n; i++) r[i] = a + i * step;
        r[n - 1] = b;
        return r;
    }

    public static float[] fade(float[] in, int sr, double msIn, double msOut) {
        float[] a = in.clone();
        int ni = Math.min(a.length, (int) (sr * msIn / 1000)), no = Math.min(a.length, (int) (sr * msOut / 1000));
        if (ni > 1) {
            double[] w = linspace(0, Math.PI / 2, ni);
            for (int i = 0; i < ni; i++) a[i] = (float) (a[i] * Math.pow(Math.sin(w[i]), 2));
        }
        if (no > 1) {
            double[] w = linspace(0, Math.PI / 2, no);
            int o = a.length - no;
            for (int i = 0; i < no; i++) a[o + i] = (float) (a[o + i] * Math.pow(Math.cos(w[i]), 2));
        }
        return a;
    }

    public static float[] trim(float[] a, int sr) {
        return trim(a, sr, -48.0, 40.0, 90.0);
    }

    static float[] trim(float[] a, int sr, double threshDb, double preMs, double postMs) {
        if (a.length == 0) return a;
        int hop = Math.max(1, (int) (sr * 0.005));
        int n = a.length / hop;
        if (n < 2) return a;
        double[] e = new double[n];
        for (int i = 0; i < n; i++) {
            double s = 0;
            for (int j = 0; j < hop; j++) {
                double v = a[i * hop + j];
                s += v * v;
            }
            e[i] = Math.sqrt(s / hop + 1e-12);
        }
        double ref = percentile(e, 95);
        int first = -1, last = -1;
        for (int i = 0; i < n; i++) {
            if (20 * Math.log10(e[i] / (ref + 1e-12) + 1e-12) > threshDb) {
                if (first < 0) first = i;
                last = i;
            }
        }
        if (first < 0) return a;
        int s = Math.max(0, first * hop - (int) (sr * preMs / 1000));
        int t = Math.min(a.length, (last + 1) * hop + (int) (sr * postMs / 1000));
        return Arrays.copyOfRange(a, s, t);
    }

    /** numpy.percentile, default 'linear' method. */
    static double percentile(double[] x, double q) {
        double[] s = x.clone();
        Arrays.sort(s);
        double idx = q / 100.0 * (s.length - 1);
        int lo = (int) Math.floor(idx);
        int hi = Math.min(s.length - 1, lo + 1);
        double f = idx - lo;
        return s[lo] + (s[hi] - s[lo]) * f;
    }

    /** Join streamed decoder blocks with a tiny equal-power crossfade. */
    public static float[] crossfadeConcat(List<float[]> blocks, int sr, double ms) {
        float[] out = null;
        int n = (int) (sr * ms / 1000);
        for (float[] b : blocks) {
            if (b.length == 0) continue;
            if (out == null) {
                out = b;
                continue;
            }
            int k = Math.min(n, Math.min(out.length, b.length));
            float[] r = new float[out.length + b.length - (k < 2 ? 0 : k)];
            if (k < 2) {
                System.arraycopy(out, 0, r, 0, out.length);
                System.arraycopy(b, 0, r, out.length, b.length);
            } else {
                double[] w = linspace(0, Math.PI / 2, k);
                int o = out.length - k;
                System.arraycopy(out, 0, r, 0, o);
                for (int i = 0; i < k; i++) r[o + i] = (float) (out[o + i] * Math.cos(w[i]) + b[i] * Math.sin(w[i]));
                System.arraycopy(b, k, r, out.length, b.length - k);
            }
            out = r;
        }
        return out == null ? new float[0] : out;
    }

    public static float[] gap(int sr, double seconds) {
        return new float[(int) (sr * Math.max(0.0, seconds))];
    }

    // ------------------------------------------------------------ EQ curve

    private static double bellDb(double f, double f0, double q, double g) {
        double bw = 1.0 / Math.max(q, 1e-3);
        double x = log2(Math.max(f, 1.0) / f0) / bw;
        return g * Math.exp(-0.5 * (2.0 * x) * (2.0 * x));
    }

    private static double shelfDb(double f, double f0, double g) {
        double x = log2(Math.max(f, 1.0) / f0);
        return g / (1.0 + Math.exp(-4.0 * x));
    }

    private static double log2(double v) {
        return Math.log(v) / Math.log(2);
    }

    static double eqCurve(double f, double highpass, double[][] bells, double[] hishelf) {
        double db = 0;
        for (double[] b : bells) db += bellDb(f, b[0], b[1], b[2]);
        if (hishelf != null) db += shelfDb(f, hishelf[0], hishelf[1]);
        double mag = Math.pow(10, db / 20);
        if (highpass > 0) {
            double r = Math.pow(Math.max(f, 1e-3) / highpass, 2);
            mag *= r / Math.sqrt(1 + r * r);
        }
        return mag;
    }

    // ------------------------------------------------------------ STFT EQ + de-esser

    static float[] eqDeess(float[] x, int sr) {
        if (x.length < 256) return x;
        final int n = 1024, hop = 256, nb = n / 2 + 1;
        float[] w = new float[n];
        for (int i = 0; i < n; i++) w[i] = (float) (0.5 - 0.5 * Math.cos(2 * Math.PI * i / (n - 1)));
        int nPad = x.length + 2 * n;
        float[] pad = new float[nPad];
        System.arraycopy(x, 0, pad, n, x.length);
        int frames = 1 + (nPad - n) / hop;
        double[][] re = new double[frames][nb], im = new double[frames][nb];
        Fft fft = Fft.of(n);
        double[] fr = new double[n], fi = new double[n];
        double[] mag = new double[nb];
        boolean[] band = new boolean[nb];
        for (int k = 0; k < nb; k++) {
            double f = k * (double) sr / n;
            mag[k] = eqCurve(f, HIGHPASS, BELLS, HISHELF);
            band[k] = f >= 5000 && f <= 10000;
        }
        double[] tot = new double[frames], bandE = new double[frames];
        for (int t = 0; t < frames; t++) {
            for (int i = 0; i < n; i++) {
                fr[i] = (float) (pad[t * hop + i] * w[i]);
                fi[i] = 0;
            }
            fft.transform(fr, fi, false);
            for (int k = 0; k < nb; k++) {
                re[t][k] = fr[k] * mag[k];
                im[t][k] = fi[k] * mag[k];
                double p = re[t][k] * re[t][k] + im[t][k] * im[t][k];
                tot[t] += p;
                if (band[k]) bandE[t] += p;
            }
        }
        if (DEESS_DB > 0) {
            double mx = 0;
            for (double v : tot) mx = Math.max(mx, v);
            double[] over = new double[frames];
            for (int t = 0; t < frames; t++) {
                double ratio = bandE[t] / (tot[t] + 1e-12);
                boolean loud = tot[t] > 1e-4 * mx;
                over[t] = loud ? Math.min(1, Math.max(0, (ratio - 0.35) / (1 - 0.35))) : 0;
            }
            for (int t = 0; t < frames; t++) {
                double s = over[t] + (t > 0 ? over[t - 1] : 0) + (t + 1 < frames ? over[t + 1] : 0);
                double g = Math.pow(10, -DEESS_DB * (s / 3) / 20);
                for (int k = 0; k < nb; k++) {
                    if (band[k]) {
                        re[t][k] *= g;
                        im[t][k] *= g;
                    }
                }
            }
        }
        double[] out = new double[nPad], norm = new double[nPad];
        for (int t = 0; t < frames; t++) {
            irfft(fft, re[t], im[t], n, fr, fi);
            int o = t * hop;
            for (int i = 0; i < n; i++) {
                out[o + i] += fr[i] * w[i];
                norm[o + i] += (double) w[i] * w[i];
            }
        }
        float[] y = new float[x.length];
        for (int i = 0; i < x.length; i++) y[i] = (float) (out[n + i] / Math.max(norm[n + i], 1e-6));
        return y;
    }

    /** numpy irfft(X, n): Hermitian-extend half spectrum, inverse FFT, real part. Result in fr. */
    private static void irfft(Fft fft, double[] re, double[] im, int n, double[] fr, double[] fi) {
        int nb = n / 2 + 1;
        for (int k = 0; k < n; k++) {
            if (k < nb) {
                fr[k] = re[k];
                fi[k] = im[k];
            } else {
                fr[k] = re[n - k];
                fi[k] = -im[n - k];
            }
        }
        fi[0] = 0;
        if (n % 2 == 0) fi[n / 2] = 0;
        fft.transform(fr, fi, true);
        for (int i = 0; i < n; i++) fr[i] /= n;
    }

    // ------------------------------------------------------------ loudness

    /** Approximate integrated loudness ("LUFS"), as dsp.loudness; NaN = None. */
    static double loudness(float[] x, int sr) {
        int len = x.length;
        if (len < sr / 10) return Double.NaN;
        Fft fft = Fft.of(len);
        double[] re = new double[len], im = new double[len];
        for (int i = 0; i < len; i++) re[i] = x[i];
        fft.transform(re, im, false);
        int nb = len / 2 + 1;
        double[] hr = new double[nb], hi = new double[nb];
        for (int k = 0; k < nb; k++) {
            double f = k * (double) sr / len;
            double g = Math.pow(10, shelfDb(f, 1500, 4.0) / 20) * eqCurve(f, 60, new double[0][], null);
            hr[k] = re[k] * g;
            hi[k] = im[k] * g;
        }
        irfft(fft, hr, hi, len, re, im);
        int b = (int) (0.4 * sr), hop = b / 4;
        double[] e;
        if (len < b) {
            double s = 0;
            for (int i = 0; i < len; i++) s += re[i] * re[i];
            e = new double[] {s / len};
        } else {
            int cnt = (len - b) / hop + 1;
            e = new double[cnt];
            for (int c = 0; c < cnt; c++) {
                double s = 0;
                for (int i = c * hop; i < c * hop + b; i++) s += re[i] * re[i];
                e[c] = s / b;
            }
        }
        double gate = Math.pow(10, -70 / 10.0);
        double sum = 0;
        int m = 0;
        for (double v : e) {
            if (v > gate) {
                sum += v;
                m++;
            }
        }
        if (m == 0) return Double.NaN;
        double rel = sum / m * Math.pow(10, -10 / 10.0);
        double s2 = 0;
        int m2 = 0;
        for (double v : e) {
            if (v > gate && v > rel) {
                s2 += v;
                m2++;
            }
        }
        return -0.691 + 10 * Math.log10(s2 / m2);
    }

    // ------------------------------------------------------------ limiter

    static double[] limit(double[] y, int sr, double ceiling) {
        int w = Math.max(3, (int) (4.0 * sr / 1000) | 1);
        int n = y.length;
        if (n == 0) return y;
        double mx = 0;
        for (double v : y) mx = Math.max(mx, Math.abs(v));
        if (mx <= ceiling) return y;
        int h = w / 2;
        // sliding max of |y| over a centred window (edge padded)
        double[] peak = slide(n, h, h, i -> Math.abs(y[i]), true);
        double[] g = new double[n];
        for (int i = 0; i < n; i++) g[i] = Math.min(1.0, ceiling / Math.max(peak[i], 1e-9));
        int r = Math.max(3, (int) (60.0 * sr / 1000) | 1);
        final double[] g0 = g;
        g = slide(n, r - 1, 0, i -> g0[i], false);
        // moving average, width w, edge padded
        double[] gs = new double[n];
        double acc = 0;
        final double[] g1 = g;
        java.util.function.IntToDoubleFunction at = i -> g1[Math.max(0, Math.min(n - 1, i))];
        for (int i = -h; i <= h; i++) acc += at.applyAsDouble(i);
        for (int i = 0; i < n; i++) {
            gs[i] = acc / w;
            acc += at.applyAsDouble(i + h + 1) - at.applyAsDouble(i - h);
        }
        double[] out = new double[n];
        for (int i = 0; i < n; i++) out[i] = Math.max(-ceiling, Math.min(ceiling, y[i] * gs[i]));
        return out;
    }

    /** Sliding max (or min) over [i-left, i+right] with edge padding, via a monotonic deque. */
    private static double[] slide(int n, int left, int right, java.util.function.IntToDoubleFunction v, boolean max) {
        double[] out = new double[n];
        ArrayDeque<double[]> dq = new ArrayDeque<>(); // {value, position}
        int next = -left; // next padded position to push
        for (int i = 0; i < n; i++) {
            while (next <= i + right) {
                double val = v.applyAsDouble(Math.max(0, Math.min(n - 1, next)));
                while (!dq.isEmpty() && (max ? dq.peekLast()[0] <= val : dq.peekLast()[0] >= val)) dq.pollLast();
                dq.addLast(new double[] {val, next});
                next++;
            }
            while (dq.peekFirst()[1] < i - left) dq.pollFirst();
            out[i] = dq.peekFirst()[0];
        }
        return out;
    }

    // ------------------------------------------------------------ polish

    /** Result of {@link #polish}: audio + the gain used (the next sentence's ref gain). */
    public static final class Polished {
        public final float[] audio;
        public final Double gainDb;

        Polished(float[] a, Double g) {
            audio = a;
            gainDb = g;
        }
    }

    /** dsp.polish with the "lillith" preset. refGainDb: utterance gain from the first sentence (null = measure). */
    public static Polished polish(float[] raw, int sr, Double refGainDb) {
        float[] a = trim(raw, sr);
        if (a.length == 0) return new Polished(a, refGainDb);
        float[] y = eqDeess(a, sr);
        if (refGainDb == null) {
            double l = loudness(y, sr);
            refGainDb = Double.isNaN(l) ? 0.0 : Math.max(-18, Math.min(18, LUFS - l));
        }
        double gain = Math.pow(10, refGainDb / 20);
        double[] d = new double[y.length];
        for (int i = 0; i < y.length; i++) d[i] = y[i] * gain;
        d = limit(d, sr, Math.pow(10, TRUE_PEAK / 20));
        float[] f = new float[d.length];
        for (int i = 0; i < d.length; i++) f[i] = (float) d[i];
        return new Polished(fade(f, sr, 8.0, 12.0), refGainDb);
    }

    // ------------------------------------------------------------ FFT (radix-2 + Bluestein for any n)

    static final class Fft {
        private final int n;
        private final int m; // power of two used (== n if n is a power of two)
        private final double[] cos, sin; // twiddles for size m
        private final double[] wr, wi; // Bluestein chirp (n)
        private final double[] br, bi; // FFT of conj chirp (m)

        private static final java.util.Map<Integer, Fft> CACHE = new java.util.HashMap<>();

        static synchronized Fft of(int n) {
            Fft f = CACHE.get(n);
            if (f == null) {
                f = new Fft(n);
                if (n <= 4096) CACHE.put(n, f);
            }
            return f;
        }

        private Fft(int n) {
            this.n = n;
            boolean pow2 = (n & (n - 1)) == 0;
            int mm = 1;
            while (mm < (pow2 ? n : 2 * n - 1)) mm <<= 1;
            m = mm;
            cos = new double[m / 2];
            sin = new double[m / 2];
            for (int i = 0; i < m / 2; i++) {
                cos[i] = Math.cos(2 * Math.PI * i / m);
                sin[i] = Math.sin(2 * Math.PI * i / m);
            }
            if (pow2) {
                wr = wi = br = bi = null;
            } else {
                wr = new double[n];
                wi = new double[n];
                for (int k = 0; k < n; k++) {
                    long kk = ((long) k * k) % (2L * n);
                    double a = Math.PI * kk / n;
                    wr[k] = Math.cos(a);
                    wi[k] = -Math.sin(a); // exp(-i*pi*k^2/n)
                }
                br = new double[m];
                bi = new double[m];
                br[0] = wr[0];
                bi[0] = -wi[0];
                for (int k = 1; k < n; k++) {
                    br[k] = br[m - k] = wr[k];
                    bi[k] = bi[m - k] = -wi[k];
                }
                radix2(br, bi, false);
            }
        }

        /** In-place complex DFT of length n (inverse = unscaled, sign +). */
        void transform(double[] re, double[] im, boolean inverse) {
            if (wr == null) {
                radix2(re, im, inverse);
                return;
            }
            if (inverse) { // IDFT(x) = conj(DFT(conj(x)))
                for (int i = 0; i < n; i++) im[i] = -im[i];
                transform(re, im, false);
                for (int i = 0; i < n; i++) im[i] = -im[i];
                return;
            }
            double[] ar = new double[m], ai = new double[m];
            for (int k = 0; k < n; k++) {
                ar[k] = re[k] * wr[k] - im[k] * wi[k];
                ai[k] = re[k] * wi[k] + im[k] * wr[k];
            }
            radix2(ar, ai, false);
            for (int k = 0; k < m; k++) {
                double r = ar[k] * br[k] - ai[k] * bi[k];
                double i = ar[k] * bi[k] + ai[k] * br[k];
                ar[k] = r;
                ai[k] = i;
            }
            radix2(ar, ai, true);
            for (int k = 0; k < n; k++) {
                double r = ar[k] / m, i = ai[k] / m;
                re[k] = r * wr[k] - i * wi[k];
                im[k] = r * wi[k] + i * wr[k];
            }
        }

        private void radix2(double[] re, double[] im, boolean inverse) {
            int len = m;
            for (int i = 1, j = 0; i < len; i++) {
                int bit = len >> 1;
                for (; (j & bit) != 0; bit >>= 1) j ^= bit;
                j ^= bit;
                if (i < j) {
                    double t = re[i];
                    re[i] = re[j];
                    re[j] = t;
                    t = im[i];
                    im[i] = im[j];
                    im[j] = t;
                }
            }
            for (int size = 2; size <= len; size <<= 1) {
                int half = size >> 1, step = len / size;
                for (int s = 0; s < len; s += size) {
                    for (int k = 0; k < half; k++) {
                        double c = cos[k * step], sn = inverse ? sin[k * step] : -sin[k * step];
                        int a = s + k, b = a + half;
                        double tr = re[b] * c - im[b] * sn, ti = re[b] * sn + im[b] * c;
                        re[b] = re[a] - tr;
                        im[b] = im[a] - ti;
                        re[a] += tr;
                        im[a] += ti;
                    }
                }
            }
        }
    }
}
