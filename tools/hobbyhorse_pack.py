"""Build a data-pack override that reskins the Croquet Mallet as a hobby horse.

Everything is read from files you supply; no game or third-party art lives in this repository.

  python tools/hobbyhorse_pack.py --obj horse.obj --texture horse_diffuse.png \
      --template <extracted>/models/weapons/mallet/notmoving.tan --out zzz_hobbyhorse.pk3

Drop the resulting pk3 next to your game's PK3 files. Packs load alphabetically and later packs
override earlier ones, so the `zzz_` name replaces models/weapons/mallet/{notmoving.tan,skin01.ftx}.
Delete the pk3 to get the original mallet back.

The TAN writer reuses the header and tags of the original mallet file (a one-frame, one-surface
weapon model) and rebuilds only the surface block, so the layout stays the one the loader
validates (src/tan.rs).
"""
import argparse
import struct
import zipfile
from io import BytesIO

import numpy as np
from PIL import Image


def read_obj(path):
    pos, uvs, faces = [], [], []
    for line in open(path, errors="ignore"):
        p = line.split()
        if not p:
            continue
        if p[0] == "v":
            pos.append([float(x) for x in p[1:4]])
        elif p[0] == "vt":
            uvs.append([float(x) for x in p[1:3]])
        elif p[0] == "f":
            corner = []
            for tok in p[1:]:
                a = tok.split("/")
                v = int(a[0]) - 1 if int(a[0]) > 0 else len(pos) + int(a[0])
                t = (int(a[1]) - 1 if int(a[1]) > 0 else len(uvs) + int(a[1])) if len(a) > 1 and a[1] else 0
                corner.append((v, t))
            for i in range(1, len(corner) - 1):
                faces.append((corner[0], corner[i], corner[i + 1]))
    return np.array(pos), np.array(uvs), faces


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--obj", required=True)
    ap.add_argument("--texture", required=True)
    ap.add_argument("--template", required=True, help="original models/weapons/mallet/notmoving.tan")
    ap.add_argument("--out", required=True)
    ap.add_argument("--length", type=float, default=72.0, help="whole-model length in game units")
    ap.add_argument("--grip", type=float, default=0.30, help="right-hand grip as a fraction of the length from the butt")
    ap.add_argument("--left-hand", type=float, default=-14.0, help="off-hand position along the shaft (weapon +X, game units)")
    ap.add_argument("--tex-size", type=int, default=512)
    ap.add_argument("--flip-v", type=int, default=1)
    ap.add_argument("--roll", type=float, default=0.0, help="degrees of roll about the shaft")
    ap.add_argument("--stick-axis", default="y", choices="xyz")
    ap.add_argument("--face-axis", default="-x", choices=["x","y","z","-x","-y","-z"], help="OBJ axis the muzzle points along")
    args = ap.parse_args()

    pos, uv, faces = read_obj(args.obj)
    ax = {"x": 0, "y": 1, "z": 2}
    s, f = ax[args.stick_axis], ax[args.face_axis.lstrip("-")]
    face_sign = -1.0 if args.face_axis.startswith("-") else 1.0
    third = ({0, 1, 2} - {s, f}).pop()
    # Shaft along +x (butt behind, head ahead), muzzle along weapon +y, remainder along weapon +z.
    # Choose the third axis sign so the transform stays a rotation (no mirroring).
    basis = np.zeros((3, 3))
    basis[0, s] = 1
    basis[1, f] = face_sign
    basis[2, third] = 1
    if np.linalg.det(basis) < 0:
        basis[2, third] = -1
    local = pos @ basis.T
    lo, hi = local[:, 0].min(), local[:, 0].max()
    scale = args.length / (hi - lo)
    local = (local - np.array([lo + (hi - lo) * args.grip, 0, 0])) * scale
    # Centre the shaft on the hand axis.
    shaft = local[local[:, 0] < local[:, 0].min() + 0.3 * args.length]
    local[:, 1:] -= np.median(shaft[:, 1:], axis=0)
    if args.roll:
        r = np.radians(args.roll)
        c, sn = np.cos(r), np.sin(r)
        local[:, 1:] = local[:, 1:] @ np.array([[c, sn], [-sn, c]])

    # Unique (position, uv) pairs become vertices.
    index, verts, tex, tris = {}, [], [], []
    for tri in faces:
        row = []
        for key in tri:
            if key not in index:
                index[key] = len(verts)
                verts.append(local[key[0]])
                u, v = uv[key[1]] if len(uv) else (0, 0)
                tex.append((u, 1 - v if args.flip_v else v))
            row.append(index[key])
        tris.append(row)
    verts = np.array(verts, dtype=np.float64)
    nv, nt = len(verts), len(tris)
    if nv > 30000 or nt > 60000:
        raise SystemExit(f"{nv} vertices / {nt} triangles exceeds the TAN loader limits; decimate first")

    template = bytearray(open(args.template, "rb").read())
    I = lambda o: struct.unpack_from("<i", template, o)[0]
    assert template[:4] == b"TAN " and I(4) == 2 and I(72) == 1 and I(80) == 1, "template must be a one-frame, one-surface TAN"
    os_ = I(104)
    head = bytearray(template[:os_])

    vmin, vmax = verts.min(axis=0), verts.max(axis=0)
    span = np.maximum(vmax - vmin, 1e-6)
    qscale = span / 65535.0
    frame = I(100)
    struct.pack_into("<3f", head, frame, *vmin)
    struct.pack_into("<3f", head, frame + 12, *vmax)
    struct.pack_into("<3f", head, frame + 24, *qscale)
    struct.pack_into("<3f", head, frame + 36, *vmin)
    struct.pack_into("<f", head, frame + 60, float(np.linalg.norm(np.maximum(abs(vmin), abs(vmax)))))

    # Swipe tags: tip of the head and a point along the shaft (positions only; rotation untouched).
    tag_swipe, tag_barrel = I(108), I(112)
    tip = float(vmax[0])
    struct.pack_into("<3f", head, tag_swipe + 64, tip * 0.92, 0.0, 0.0)
    struct.pack_into("<3f", head, tag_barrel + 64, tip * 0.45, 0.0, 0.0)

    ot = 104
    oc = ot + nt * 12
    ouv = oc + nv * 4
    ov = ouv + nv * 8
    end = ov + nv * 8
    surf = bytearray(end)
    surf[0:4] = b"TAN "
    name = template[os_ + 4:os_ + 68]
    surf[4:68] = name
    struct.pack_into("<7i", surf, 68, 1, nv, nv, nt, ot, oc, ouv)
    struct.pack_into("<2i", surf, 96, ov, end)
    for i, t in enumerate(tris):
        struct.pack_into("<3i", surf, ot + i * 12, *t)
    # Identity collapse map (indices must be < nv): disables progressive detail.
    for i in range(nv):
        struct.pack_into("<i", surf, oc + i * 4, i)
    for i, (u, v) in enumerate(tex):
        struct.pack_into("<2f", surf, ouv + i * 8, float(u), float(v))
    q = np.rint((verts - vmin) / qscale).clip(0, 65535).astype(np.uint16)
    for i in range(nv):
        struct.pack_into("<3H", surf, ov + i * 8, *q[i])

    data = bytes(head) + bytes(surf)
    data = bytearray(data)
    struct.pack_into("<i", data, 172, len(data))

    img = Image.open(args.texture).convert("RGB").resize((args.tex_size, args.tex_size), Image.LANCZOS)
    rgba = np.dstack([np.asarray(img), np.full((args.tex_size, args.tex_size), 255, np.uint8)])
    ftx = struct.pack("<3i", args.tex_size, args.tex_size, 0) + rgba.tobytes()

    with zipfile.ZipFile(args.out, "w", zipfile.ZIP_DEFLATED) as z:
        z.writestr("models/weapons/mallet/notmoving.tan", bytes(data))
        z.writestr("models/weapons/mallet/skin01.ftx", ftx)
        z.writestr("models/weapons/mallet/twohand.txt", f"{args.left_hand}\n")
    print(f"{args.out}: {nv} vertices, {nt} triangles, length {args.length} units, "
          f"extents x {vmin[0]:.1f}..{vmax[0]:.1f} y {vmin[1]:.1f}..{vmax[1]:.1f} z {vmin[2]:.1f}..{vmax[2]:.1f}")


if __name__ == "__main__":
    main()
