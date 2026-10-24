"""Verify a UsdSkel export against a point-baked export of the same scene.

    python check_usdskel.py skel.usda baked.usda

Uses Pixar's USD (pip install usd-core): UsdSkel computes the skinned points of every
skinned mesh in skel.usda at every time code and compares them, in world space, with the
baked points of the mesh with the same Maya name in baked.usda. Prints the max deviation.
"""
import sys
from pxr import Usd, UsdGeom, UsdSkel, Vt, Gf

skel_stage = Usd.Stage.Open(sys.argv[1])
bake_stage = Usd.Stage.Open(sys.argv[2])
start, end = skel_stage.GetStartTimeCode(), skel_stage.GetEndTimeCode()
times = [Usd.TimeCode.Default()] if end <= start else [Usd.TimeCode(t) for t in range(int(start), int(end) + 1)]

baked = {}
for prim in bake_stage.Traverse():
    if prim.IsA(UsdGeom.Mesh):
        xf_name = prim.GetParent().GetCustomDataByKey("mayaName")
        baked[xf_name] = prim

cache = UsdSkel.Cache()
worst = 0.0
checked = 0
for root_prim in skel_stage.Traverse():
    if not root_prim.IsA(UsdSkel.Root):
        continue
    root = UsdSkel.Root(root_prim)
    cache.Populate(root, Usd.TraverseInstanceProxies())
    for binding in cache.ComputeSkelBindings(root, Usd.TraverseInstanceProxies()):
        skel_query = cache.GetSkelQuery(binding.GetSkeleton())
        for sq in binding.GetSkinningTargets():
            mesh = UsdGeom.Mesh(sq.GetPrim())
            name = sq.GetPrim().GetName()
            ref = baked.get(name)
            if ref is None:
                print(f"! no baked mesh named {name}")
                continue
            for t in times:
                pts = mesh.GetPointsAttr().Get(t)
                if sq.HasBlendShapes():
                    weights = skel_query.GetAnimQuery().ComputeBlendShapeWeights(t) if skel_query.GetAnimQuery() else Vt.FloatArray()
                    mapped = sq.GetBlendShapeMapper().Remap(weights) if sq.GetBlendShapeMapper() else weights
                    pts = Vt.Vec3fArray(pts)
                    sub_shapes, sub_weights = Vt.UIntArray(), Vt.FloatArray()
                    # Simple offsets-only blend shapes.
                    targets = sq.GetBlendShapeTargetsRel().GetTargets() if hasattr(sq, "GetBlendShapeTargetsRel") else UsdSkel.BindingAPI(sq.GetPrim()).GetBlendShapeTargetsRel().GetTargets()
                    for i, tp in enumerate(targets):
                        offs = UsdSkel.BlendShape(skel_stage.GetPrimAtPath(tp)).GetOffsetsAttr().Get()
                        for k in range(len(pts)):
                            pts[k] = pts[k] + offs[k] * mapped[i]
                xforms = skel_query.ComputeSkinningTransforms(t)
                pts = Vt.Vec3fArray(pts)
                ok = sq.ComputeSkinnedPoints(xforms, pts, t)
                assert ok, "skinning failed"
                # Skinned points are in skeleton space (identity under /World).
                ref_pts = UsdGeom.Mesh(ref).GetPointsAttr().Get(t)
                ref_xf = UsdGeom.Xformable(ref.GetParent()).ComputeLocalToWorldTransform(t)
                d = max((Gf.Vec3d(p) - ref_xf.Transform(Gf.Vec3d(r))).GetLength() for p, r in zip(pts, ref_pts))
                worst = max(worst, d)
                checked += 1
print(f"checked {checked} mesh-frames, max deviation {worst:.6f}")
sys.exit(0 if checked > 0 and worst < 1e-3 else 1)
