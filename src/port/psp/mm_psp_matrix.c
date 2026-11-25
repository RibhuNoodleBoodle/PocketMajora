#include "ultra64.h"
#if PLATFORM_PSP
#include "oot_psp_frame_interpolation.h"
#endif

static inline void MmPsp_GuMtxF2LPack(const f32 mf[4][4], Mtx* m) {
    s32 i, j;
    s32 e1, e2;
    s32 *ai, *af;

    ai = (s32*)&m->m[0][0];
    af = (s32*)&m->m[2][0];

    for (i = 0; i < 4; i++) {
        for (j = 0; j < 2; j++) {
            e1 = FTOFIX32(mf[i][j * 2]);
            e2 = FTOFIX32(mf[i][j * 2 + 1]);
            *(ai++) = (e1 & 0xFFFF0000) | ((e2 >> 16) & 0xFFFF);
            *(af++) = ((e1 << 16) & 0xFFFF0000) | (e2 & 0xFFFF);
        }
    }
}

#if PLATFORM_PSP
void OotPsp_GuMtxF2LWithSite(const float mf[4][4], void* m, const void* site) {
    OotPspFrameInterpolation_Record(m, mf, site);
    MmPsp_GuMtxF2LPack(mf, (Mtx*)m);
}
#endif

void guMtxF2L(f32 mf[4][4], Mtx* m) {
#if PLATFORM_PSP
    OotPsp_GuMtxF2LWithSite(mf, m, __builtin_return_address(0));
#else
    MmPsp_GuMtxF2LPack(mf, m);
#endif
}

