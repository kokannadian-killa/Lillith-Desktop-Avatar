"""White-background key: flood-fill near-white from the border, soft edge alpha
by un-mixing against white. Returns RGBA uint8."""
import numpy as np
from scipy import ndimage

def key_white(rgb, hard=242, soft=200, hole_min=150, matte=None):
    rgb = rgb.astype(np.float32)
    mn = rgb.min(axis=2)
    # background candidates: near white
    cand = mn >= hard
    lab, n = ndimage.label(cand)
    border = np.unique(np.concatenate([lab[0], lab[-1], lab[:, 0], lab[:, -1]]))
    border = border[border > 0]
    bg = np.isin(lab, border)
    # enclosed holes (e.g. between tail and leg): big, very pure white blobs
    if n:
        ids = np.arange(1, n + 1)
        sizes = ndimage.sum(cand, lab, ids)
        means = ndimage.mean(mn, lab, ids)
        sel = (sizes >= hole_min) & (means >= 251)
        if matte is not None:  # matte (0..255) says background -> also a hole
            cand2 = mn >= 235
            lab, n = ndimage.label(cand2)
            ids2 = np.arange(1, n + 1)
            msz = ndimage.sum(cand2, lab, ids2)
            mmean = ndimage.mean(matte.astype(np.float32), lab, ids2)
            bg |= np.isin(lab, ids2[(msz >= 20) & (mmean < 30)])
            lab, n = ndimage.label(cand)
        hole = ids[sel]
        bg |= np.isin(lab, hole)
    # edge band: pixels adjacent to bg that are lightish -> partial alpha
    near = ndimage.binary_dilation(bg, iterations=3) & ~bg
    alpha = np.ones(mn.shape, np.float32)
    alpha[bg] = 0.0
    a_edge = np.clip((255.0 - mn) / (255.0 - soft), 0, 1)
    alpha[near] = np.minimum(alpha[near], a_edge[near])
    # drop tiny speckle islands (compression noise far from body)
    fg = alpha > 0.05
    lab2, n2 = ndimage.label(fg)
    if n2 > 1:
        sizes = ndimage.sum(fg, lab2, range(1, n2 + 1))
        keep = np.zeros(n2 + 1, bool); keep[1:] = sizes >= 40
        alpha[~keep[lab2]] = 0
    # un-mix white from colour
    a = np.clip(alpha, 1e-3, 1)[..., None]
    col = np.clip((rgb - (1 - a) * 255.0) / a, 0, 255)
    out = np.dstack([col, alpha[..., None] * 255]).round().astype(np.uint8)
    out[alpha <= 0] = 0
    return out


def key_combined(rgb, mask_floor=0.5, dilate=2):
    """White key, then cut regions the isnet-anime matte calls background
    (motion-blur ghosts, enclosed white slivers). Keeps the white key's soft edges.
    The matte comes from matte_model.isnet_mask (onnxruntime + the bundled isnet-anime.onnx; same mask rembg
    gives, nothing downloaded). A missing model / package raises matte_model.MatteModelError with the fix."""
    import matte_model
    mat = matte_model.isnet_mask(rgb)
    out = key_white(rgb, matte=mat)
    m = mat.astype(np.float32) / 255
    m = ndimage.grey_dilation(m, size=(2 * dilate + 1, 2 * dilate + 1))
    keep = np.clip((m - (mask_floor - 0.25)) / 0.5, 0, 1)
    a = out[..., 3].astype(np.float32) * keep
    out[..., 3] = a.round().astype(np.uint8)
    out[a <= 0] = 0
    return out
