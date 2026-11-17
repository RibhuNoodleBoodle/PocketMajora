#ifndef MM_PSP_DIAG_LOG_H
#define MM_PSP_DIAG_LOG_H

#ifdef __cplusplus
extern "C" {
#endif

void MmPsp_DiagLogInit(void);
void MmPsp_DiagLog(const char* fmt, ...);

#ifdef __cplusplus
}
#endif

#endif
