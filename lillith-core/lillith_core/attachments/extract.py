"""Content extraction: text, PDF (pdftotext), images (dims, thumbnail, OCR).

Optional deps (Pillow, pytesseract) are guarded; CLI tools are used when present.
Nothing here touches the network.
"""
from __future__ import annotations

import mimetypes
import shutil
import struct
import subprocess
from pathlib import Path

try:  # optional
    from PIL import Image  # type: ignore
except Exception:  # pragma: no cover
    Image = None

try:  # optional
    import pytesseract  # type: ignore
except Exception:  # pragma: no cover
    pytesseract = None

TEXT_EXTS = {
    ".txt", ".md", ".markdown", ".rst", ".log", ".csv", ".tsv", ".json", ".jsonl",
    ".yaml", ".yml", ".toml", ".ini", ".cfg", ".conf", ".xml", ".html", ".htm",
    ".py", ".js", ".ts", ".tsx", ".jsx", ".c", ".h", ".cpp", ".hpp", ".rs", ".go",
    ".java", ".kt", ".rb", ".php", ".sh", ".bash", ".fish", ".zsh", ".lua", ".sql",
    ".css", ".scss", ".gd", ".cs", ".swift", ".pl", ".r", ".m", ".tex", ".srt", ".vtt",
}
IMAGE_EXTS = {".png", ".jpg", ".jpeg", ".gif", ".webp", ".bmp", ".tif", ".tiff"}

mimetypes.add_type("text/markdown", ".md")
mimetypes.add_type("text/x-fish", ".fish")
mimetypes.add_type("text/x-toml", ".toml")
mimetypes.add_type("image/webp", ".webp")


def sniff_mime(path: Path, head: bytes | None = None) -> str:
    if head is None:
        with open(path, "rb") as f:
            head = f.read(4096)
    if head.startswith(b"%PDF-"):
        return "application/pdf"
    if head.startswith(b"\x89PNG\r\n\x1a\n"):
        return "image/png"
    if head.startswith(b"\xff\xd8\xff"):
        return "image/jpeg"
    if head[:6] in (b"GIF87a", b"GIF89a"):
        return "image/gif"
    if head[:4] == b"RIFF" and head[8:12] == b"WEBP":
        return "image/webp"
    if head[:2] == b"BM":
        return "image/bmp"
    ext = path.suffix.lower()
    guess = mimetypes.guess_type(path.name)[0]
    if ext in TEXT_EXTS:
        return guess if guess and (guess.startswith("text/") or "json" in guess or "xml" in guess) else "text/plain"
    if guess:
        return guess
    if looks_like_text(head):
        return "text/plain"
    return "application/octet-stream"


def looks_like_text(head: bytes) -> bool:
    if not head:
        return True
    if b"\x00" in head:
        return False
    try:
        head.decode("utf-8")
        return True
    except UnicodeDecodeError as e:
        # tolerate a multibyte char cut at the end of the sample
        return e.start >= len(head) - 4


def kind_of(mime: str) -> str:
    if mime == "application/pdf":
        return "pdf"
    if mime.startswith("image/"):
        return "image"
    if mime.startswith("text/") or mime in ("application/json", "application/xml",
                                            "application/x-sh", "application/javascript",
                                            "application/toml", "application/x-yaml"):
        return "text"
    return "binary"


def read_text_file(path: Path, max_bytes: int) -> tuple[str, bool]:
    with open(path, "rb") as f:
        data = f.read(max_bytes + 1)
    truncated = len(data) > max_bytes
    data = data[:max_bytes]
    return data.decode("utf-8", errors="replace"), truncated


def extract_pdf(path: Path, max_bytes: int) -> tuple[str, bool, dict]:
    info: dict = {}
    exe = shutil.which("pdftotext")
    if not exe:
        return "", False, {"error": "pdftotext not installed (pacman -S poppler)"}
    try:
        out = subprocess.run([exe, "-layout", "-enc", "UTF-8", str(path), "-"],
                             capture_output=True, timeout=120, check=False)
        text = out.stdout.decode("utf-8", errors="replace")
        if out.returncode != 0 and not text:
            info["error"] = out.stderr.decode(errors="replace").strip()[:500]
    except subprocess.TimeoutExpired:
        return "", False, {"error": "pdftotext timed out"}
    pinfo = shutil.which("pdfinfo")
    if pinfo:
        try:
            o = subprocess.run([pinfo, str(path)], capture_output=True, timeout=30, text=True)
            for line in o.stdout.splitlines():
                if line.startswith("Pages:"):
                    info["pages"] = int(line.split(":", 1)[1].strip())
        except Exception:
            pass
    text = text.replace("\f", "\n")
    b = text.encode("utf-8")
    truncated = len(b) > max_bytes
    if truncated:
        text = b[:max_bytes].decode("utf-8", errors="ignore")
    return text, truncated, info


def image_size(path: Path) -> tuple[int, int] | None:
    """Image dimensions; Pillow if present, else stdlib header parsing."""
    if Image is not None:
        try:
            with Image.open(path) as im:
                return im.size
        except Exception:
            pass
    try:
        with open(path, "rb") as f:
            head = f.read(64 * 1024)
        if head.startswith(b"\x89PNG"):
            return struct.unpack(">II", head[16:24])
        if head[:6] in (b"GIF87a", b"GIF89a"):
            return struct.unpack("<HH", head[6:10])
        if head[:2] == b"BM":
            w, h = struct.unpack("<ii", head[18:26])
            return w, abs(h)
        if head[:4] == b"RIFF" and head[8:12] == b"WEBP":
            chunk = head[12:16]
            if chunk == b"VP8X":
                w = int.from_bytes(head[24:27], "little") + 1
                h = int.from_bytes(head[27:30], "little") + 1
                return w, h
            if chunk == b"VP8 ":
                w, h = struct.unpack("<HH", head[26:30])
                return w & 0x3FFF, h & 0x3FFF
            if chunk == b"VP8L":
                b = head[21:25]
                w = 1 + (((b[1] & 0x3F) << 8) | b[0])
                h = 1 + (((b[3] & 0xF) << 10) | (b[2] << 2) | ((b[1] & 0xC0) >> 6))
                return w, h
        if head.startswith(b"\xff\xd8"):
            i = 2
            while i < len(head) - 9:
                if head[i] != 0xFF:
                    i += 1
                    continue
                marker = head[i + 1]
                if marker in (0xC0, 0xC1, 0xC2, 0xC3, 0xC5, 0xC6, 0xC7, 0xC9, 0xCA, 0xCB, 0xCD, 0xCE, 0xCF):
                    h, w = struct.unpack(">HH", head[i + 5:i + 9])
                    return w, h
                seg = struct.unpack(">H", head[i + 2:i + 4])[0]
                i += 2 + seg
    except Exception:
        pass
    return None


def make_thumbnail(src: Path, dest: Path, size: int) -> bool:
    dest.parent.mkdir(parents=True, exist_ok=True)
    if Image is not None:
        try:
            with Image.open(src) as im:
                im.thumbnail((size, size))
                if im.mode not in ("RGB", "RGBA"):
                    im = im.convert("RGBA")
                im.save(dest, "PNG")
            return True
        except Exception:
            return False
    for tool in ("magick", "convert"):
        exe = shutil.which(tool)
        if exe:
            r = subprocess.run([exe, f"{src}[0]", "-thumbnail", f"{size}x{size}", str(dest)],
                               capture_output=True, timeout=60)
            return r.returncode == 0 and dest.exists()
    return False


def ocr_image(path: Path) -> str:
    if pytesseract is not None and Image is not None:
        try:
            with Image.open(path) as im:
                return pytesseract.image_to_string(im)
        except Exception:
            pass
    exe = shutil.which("tesseract")
    if exe:
        try:
            r = subprocess.run([exe, str(path), "-", "--psm", "3"], capture_output=True, timeout=120)
            if r.returncode == 0:
                return r.stdout.decode("utf-8", errors="replace")
        except Exception:
            pass
    return ""
