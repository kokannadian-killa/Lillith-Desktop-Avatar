"""Content-addressed attachment store.

Layout under ``home`` (default ~/.local/share/lillith/attachments):

    blobs/<aa>/<sha256><ext>   original bytes (deduped by sha256, read-only)
    meta/<sha256>.json         metadata record
    text/<sha256>.txt          extracted text (if any)
    thumbs/<sha256>.png        thumbnail for images
    lillith.sqlite             per-conversation attachment lists + history
"""
from __future__ import annotations

import builtins
import hashlib
import json
import os
import re
import shutil
import tempfile
import time
from dataclasses import dataclass, asdict, field
from pathlib import Path
from typing import Iterable
from urllib.parse import unquote, urlparse

from . import extract
from .config import Config
from .db import DB

ID_PREFIX = "att_"


class AttachmentError(Exception):
    pass


class NotFound(AttachmentError):
    pass


@dataclass
class Attachment:
    id: str
    name: str
    mime: str
    kind: str
    size: int
    sha256: str
    created: float
    path: str
    text_path: str | None = None
    thumbnail_path: str | None = None
    width: int | None = None
    height: int | None = None
    pages: int | None = None
    text_chars: int = 0
    text_lines: int = 0
    text_truncated: bool = False
    text_source: str | None = None   # "file" | "pdftotext" | "ocr" | None
    notes: list = field(default_factory=list)

    @property
    def uri(self) -> str:
        return Path(self.path).as_uri()

    def to_dict(self) -> dict:
        d = asdict(self)
        d["uri"] = self.uri
        d["thumbnail_uri"] = Path(self.thumbnail_path).as_uri() if self.thumbnail_path else None
        return d

    @classmethod
    def from_dict(cls, d: dict) -> "Attachment":
        known = cls.__dataclass_fields__
        return cls(**{k: v for k, v in d.items() if k in known})


def sha256_file(path: Path) -> str:
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def parse_uri_list(data: str) -> list[Path]:
    """Parse a text/uri-list (KDE/Qt drag-and-drop payload) into local paths."""
    out = []
    for line in data.splitlines():
        line = line.strip()
        if not line or line.startswith("#"):
            continue
        if line.startswith("file:"):
            p = urlparse(line)
            if p.netloc not in ("", "localhost"):
                continue  # remote file URIs are ignored (local-only)
            out.append(Path(unquote(p.path)))
        elif line.startswith("/") or line.startswith("~"):
            out.append(Path(line).expanduser())
    return out


class AttachmentStore:
    def __init__(self, config: Config | None = None):
        self.config = config or Config.load()
        self.home = self.config.home
        for sub in ("blobs", "meta", "text", "thumbs"):
            (self.home / sub).mkdir(parents=True, exist_ok=True)
        self.db = DB(self.home / "lillith.sqlite")

    def close(self):
        self.db.close()

    def __enter__(self):
        return self

    def __exit__(self, *a):
        self.close()

    def _meta_path(self, sha: str) -> Path:
        return self.home / "meta" / f"{sha}.json"

    def _blob_path(self, sha: str, ext: str) -> Path:
        return self.home / "blobs" / sha[:2] / f"{sha}{ext}"

    # ---------- ingest ----------
    def add(self, path, conversation: str | None = None, name: str | None = None) -> Attachment:
        src = Path(path).expanduser()
        if not src.is_file():
            raise AttachmentError(f"not a file: {src}")
        size = src.stat().st_size
        if size > self.config.max_file_bytes:
            raise AttachmentError(f"{src.name} is {size} bytes; limit is {self.config.max_file_bytes}")
        sha = sha256_file(src)
        existing = self._load(sha)
        if existing is not None and Path(existing.path).is_file():
            att = existing  # dedupe: identical bytes are stored once
        else:
            att = self._ingest_new(src, sha, size, name or src.name)
        if conversation:
            self.db.attach(conversation, att.id)
        return att

    def add_bytes(self, data: bytes, name: str, conversation: str | None = None) -> Attachment:
        """GUI helper: ingest pasted/dropped bytes (e.g. a clipboard image)."""
        safe = os.path.basename(name) or "pasted"
        with tempfile.TemporaryDirectory() as td:
            p = Path(td) / safe
            p.write_bytes(data)
            return self.add(p, conversation=conversation, name=safe)

    def add_many(self, paths: Iterable, conversation: str | None = None) -> list[Attachment]:
        return [self.add(p, conversation=conversation) for p in paths]

    def add_uri_list(self, uri_list: str, conversation: str | None = None) -> list[Attachment]:
        """GUI helper for drag-and-drop: accepts a text/uri-list payload."""
        return self.add_many([p for p in parse_uri_list(uri_list) if p.is_file()], conversation)

    def _ingest_new(self, src: Path, sha: str, size: int, name: str) -> Attachment:
        cfg = self.config
        mime = extract.sniff_mime(src)
        kind = extract.kind_of(mime)
        suf = src.suffix.lower()
        ext = suf if re.fullmatch(r"\.[a-z0-9]{1,10}", suf or "") else ""
        blob = self._blob_path(sha, ext)
        blob.parent.mkdir(parents=True, exist_ok=True)
        if blob.exists():
            os.chmod(blob, 0o644)
        tmp = blob.with_name(blob.name + ".part")
        shutil.copyfile(src, tmp)
        os.replace(tmp, blob)
        os.chmod(blob, 0o444)

        att = Attachment(id=ID_PREFIX + sha[:16], name=name, mime=mime, kind=kind, size=size,
                         sha256=sha, created=time.time(), path=str(blob))
        text, truncated, source = "", False, None
        if kind == "text":
            text, truncated = extract.read_text_file(blob, cfg.max_text_bytes)
            source = "file"
        elif kind == "pdf":
            text, truncated, info = extract.extract_pdf(blob, cfg.max_text_bytes)
            att.pages = info.get("pages")
            if info.get("error"):
                att.notes.append(info["error"])
            source = "pdftotext" if text.strip() else None
            if not text.strip():
                text = ""
                att.notes.append("no text layer (scanned PDF?)")
        elif kind == "image":
            dims = extract.image_size(blob)
            if dims:
                att.width, att.height = int(dims[0]), int(dims[1])
            thumb = self.home / "thumbs" / f"{sha}.png"
            if extract.make_thumbnail(blob, thumb, cfg.thumb_size):
                att.thumbnail_path = str(thumb)
            else:
                att.notes.append("thumbnail unavailable (install python-pillow)")
            if cfg.ocr:
                text = extract.ocr_image(blob).strip()
                if text:
                    source = "ocr"
                    b = text.encode()
                    if len(b) > cfg.max_text_bytes:
                        text, truncated = b[:cfg.max_text_bytes].decode(errors="ignore"), True
        if text:
            tp = self.home / "text" / f"{sha}.txt"
            tp.write_text(text, encoding="utf-8")
            att.text_path = str(tp)
            att.text_chars = len(text)
            att.text_lines = len(text.splitlines())
            att.text_truncated = truncated
            att.text_source = source
        self._save(att)
        return att

    # ---------- metadata ----------
    def _save(self, att: Attachment):
        mp = self._meta_path(att.sha256)
        tmp = mp.with_suffix(".tmp")
        tmp.write_text(json.dumps(att.to_dict(), indent=2), encoding="utf-8")
        os.replace(tmp, mp)

    def _load(self, sha: str) -> Attachment | None:
        mp = self._meta_path(sha)
        if not mp.is_file():
            return None
        return Attachment.from_dict(json.loads(mp.read_text(encoding="utf-8")))

    def resolve(self, ref) -> Attachment:
        """Find by id (att_xxx), full/partial sha256 (>=6 hex chars), or exact original name."""
        if isinstance(ref, Attachment):
            return ref
        ref = str(ref).strip()
        key = ref[len(ID_PREFIX):] if ref.startswith(ID_PREFIX) else ref
        if re.fullmatch(r"[0-9a-f]{6,64}", key):
            matches = builtins.list((self.home / "meta").glob(f"{key}*.json"))
            if len(matches) == 1:
                return self._load(matches[0].stem)
            if len(matches) > 1:
                raise AttachmentError(f"ambiguous reference {ref!r}")
        by_name = [a for a in self.list() if a.name == ref]
        if len(by_name) == 1:
            return by_name[0]
        if len(by_name) > 1:
            raise AttachmentError(f"several attachments are named {ref!r}; use the id")
        raise NotFound(f"no attachment {ref!r}")

    get = resolve

    def list(self, conversation: str | None = None) -> builtins.list:
        if conversation:
            out = []
            for aid in self.db.attachments_for(conversation):
                try:
                    out.append(self.resolve(aid))
                except NotFound:
                    pass
            return out
        atts = []
        for mp in (self.home / "meta").glob("*.json"):
            try:
                atts.append(Attachment.from_dict(json.loads(mp.read_text(encoding="utf-8"))))
            except Exception:
                continue
        return sorted(atts, key=lambda a: a.created)

    # ---------- reading ----------
    def text(self, ref) -> str:
        att = self.resolve(ref)
        if att.text_path and Path(att.text_path).is_file():
            return Path(att.text_path).read_text(encoding="utf-8", errors="replace")
        return ""

    def read(self, ref, offset: int = 1, limit: int | None = None) -> dict:
        """Paged read of extracted text. ``offset`` is a 1-based line number;
        negative counts back from the end (-1 = last line)."""
        att = self.resolve(ref)
        limit = limit or self.config.read_default_limit
        lines = self.text(att).splitlines()
        total = len(lines)
        start = max(total + offset, 0) if offset < 0 else max(offset - 1, 0)
        chunk = lines[start:start + limit]
        cap = self.config.max_line_chars
        out = []
        for i, line in enumerate(chunk, start=start + 1):
            if len(line) > cap:
                line = line[:cap] + f" …[line truncated, {len(line)} chars]"
            out.append((i, line))
        end = start + len(chunk)
        return {"id": att.id, "name": att.name, "total_lines": total, "start": start + 1,
                "end": end, "lines": out, "has_more": end < total,
                "next_offset": end + 1 if end < total else None,
                "text_available": bool(att.text_path)}

    # ---------- search ----------
    def search(self, query: str, conversation: str | None = None, regex: bool = False,
               ignore_case: bool = True, max_results: int = 200, context: int = 0) -> builtins.list:
        flags = re.IGNORECASE if ignore_case else 0
        pat = re.compile(query if regex else re.escape(query), flags)
        results = []
        for att in self.list(conversation):
            lines = self.text(att).splitlines()
            hit_any = False
            for i, line in enumerate(lines, 1):
                if pat.search(line):
                    hit_any = True
                    r = {"id": att.id, "name": att.name, "line": i, "text": line[:500]}
                    if context:
                        r["before"] = lines[max(0, i - 1 - context):i - 1]
                        r["after"] = lines[i:i + context]
                    results.append(r)
                    if len(results) >= max_results:
                        return results
            if not hit_any and pat.search(att.name):
                results.append({"id": att.id, "name": att.name, "line": 0, "text": "(filename match)"})
        return results

    # ---------- delete / export ----------
    def delete(self, ref) -> Attachment:
        att = self.resolve(ref)
        for p in (att.path, att.text_path, att.thumbnail_path):
            if p and Path(p).exists():
                try:
                    os.chmod(p, 0o644)
                except OSError:
                    pass
                Path(p).unlink()
        self._meta_path(att.sha256).unlink(missing_ok=True)
        self.db.forget_attachment(att.id)
        return att

    def attach(self, conversation: str, ref) -> Attachment:
        att = self.resolve(ref)
        self.db.attach(conversation, att.id)
        return att

    def detach(self, conversation: str, ref) -> bool:
        return self.db.detach(conversation, self.resolve(ref).id)

    def export(self, ref, dest, with_text: bool = False) -> Path:
        att = self.resolve(ref)
        dest = Path(dest).expanduser()
        if dest.is_dir():
            dest = dest / os.path.basename(att.name)
        dest.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(att.path, dest)
        os.chmod(dest, 0o644)
        if with_text and att.text_path:
            shutil.copyfile(att.text_path, dest.with_name(dest.name + ".txt"))
        return dest

    def verify(self) -> builtins.list:
        """Integrity check: missing blobs or sha256 mismatches."""
        problems = []
        for att in self.list():
            p = Path(att.path)
            if not p.is_file():
                problems.append(f"{att.id}: blob missing")
            elif sha256_file(p) != att.sha256:
                problems.append(f"{att.id}: sha256 mismatch")
        return problems
