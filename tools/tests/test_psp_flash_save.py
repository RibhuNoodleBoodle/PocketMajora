"""Run PSP flash completion through MM's real save and file-select wait states."""
from pathlib import Path
import os
import re
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[2]
game = (ROOT / 'src/port/psp/mm_psp_game.c').read_text()
sram = (ROOT / 'src/code/z_sram_NES.c').read_text()
names = (ROOT / 'src/overlays/gamestates/ovl_file_choose/z_file_nameset_NES.c').read_text()
save_header = (ROOT / 'include/z64save.h').read_text()


def function(source, name):
    start = re.search(r'^(?:static )?[^\n]*\b' + name + r'\([^;]*?\)\s*\{', source, re.M).start()
    return source[start:source.index('\n}', source.index('{', start)) + 2] + '\n'


code = r'''
#include <assert.h>
#include <stdbool.h>
#include <stdint.h>
#include <stddef.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <fcntl.h>
#include <unistd.h>
typedef uint8_t u8; typedef uint16_t u16; typedef uint32_t u32; typedef uint64_t u64;
typedef int16_t s16; typedef int32_t s32; typedef u64 OSTime;
typedef int SceUID; typedef size_t SceSize; typedef ssize_t SceSSize;
#include "PR/os_convert.h"
#define PSP_O_RDONLY O_RDONLY
#define PSP_O_WRONLY O_WRONLY
#define PSP_O_CREAT O_CREAT
#define PSP_O_TRUNC O_TRUNC
static size_t ioLimit=4093;
static int failOpen,failWrite,failClose,failRemove,failRenameNum,renameCount;
static ssize_t writeBudget=-1;
static int zeroWriteOnce;
static SceUID sceIoOpen(const char* path,int flags,int mode) {
    if (failOpen) return -1;
    return open(path,flags,mode);
}
static SceSSize sceIoRead(SceUID fd,void* data,SceSize size) {
    return read(fd,data,size>ioLimit ? ioLimit : size);
}
static SceSSize sceIoWrite(SceUID fd,const void* data,SceSize size) {
    if (failWrite) return -1;
    if (zeroWriteOnce) { zeroWriteOnce=0; return 0; }
    SceSize chunk = size>ioLimit ? ioLimit : size;
    if (writeBudget>=0) {
        if (writeBudget==0) return -1;
        if ((ssize_t)chunk > writeBudget) chunk = (SceSize)writeBudget;
        writeBudget -= (ssize_t)chunk;
    }
    return write(fd,data,chunk);
}
static int sceIoClose(SceUID fd) {
    int rc = close(fd);
    if (failClose) { failClose=0; return -1; }
    return rc;
}
static int sceIoRemove(const char* path) {
    if (failRemove) return -1;
    return remove(path);
}
static int sceIoRename(const char* oldname,const char* newname) {
    renameCount++;
    if (failRenameNum && renameCount == failRenameNum) return -1;
    /* Match PSP FAT semantics: sceIoRename fails if destination exists. */
    if (access(newname, F_OK) == 0) return -1;
    return rename(oldname,newname);
}
static int sceIoSync(const char* device, unsigned int unk) {
    (void)device; (void)unk;
    return 0;
}
static const char* OotPsp_ResolveRootPath(const char* path,char* buffer,size_t size) {
    (void)buffer;(void)size;return path;
}
static u64 nowUsec=63900000000000000ULL;
static void sceRtcGetCurrentTick(u64* ticks) { *ticks=nowUsec; }
#define SAVE_BUFFER_SIZE 0x4000
#define bzero(data,size) memset(data,0,size)
#define Lib_MemCpy memcpy
typedef struct { struct { bool isOwlSave; struct { u16 checksum; } saveInfo; } save; int fileNum; } SaveContext;
static SaveContext gSaveContext;
'''
for source, pattern in [
    (game, r'^#define MM_PSP_(?:SAVE_PATH|FLASH_SIZE|SAVE_IO_CHUNK_SIZE)[^\n]+'),
    ((ROOT / 'include/PR/os_internal_flash.h').read_text(), r'^#define FLASH_BLOCK_SIZE[^\n]+'),
    (save_header, r'^#define (?:SECONDS_TO_TIMER|OSTIME_TO_TIMER)\([^\n]+'),
    (game, r'^static (?:u8|s32) sMmPspFlash[^;]+;'),
]:
    code += '\n'.join(re.findall(pattern, source, re.M)) + '\n'
context = re.search(r'typedef struct SramContext \{.*?\} SramContext;', save_header, re.S).group()
code += context + '''
typedef struct { SramContext sramCtx; int configMode; } FileSelectState;
typedef FileSelectState GameState;
enum { CM_NAME_ENTRY_WAIT_FOR_FLASH_SAVE,CM_NAME_ENTRY_TO_MAIN,CM_OPTIONS_WAIT_FOR_FLASH_SAVE,CM_OPTIONS_TO_MAIN };
'''
code += function((ROOT / 'src/port/psp/backend/libultra_psp.c').read_text(), 'osGetTime')
for name in ('MmPsp_GetSavePath','MmPsp_FormatSaveSidecarPath','MmPsp_SavePathExists',
             'MmPsp_SyncSaveDevice','MmPsp_ReadExactSaveFile','MmPsp_WriteExactSaveFile',
             'MmPsp_LoadFlash','MmPsp_CommitFlash','SysFlashrom_IsInit',
             'SysFlashrom_InitFlash','SysFlashrom_Read','SysFlashrom_WriteAsync','SysFlashrom_IsBusy',
             'SysFlashrom_AwaitResult','SysFlashrom_WriteSync'):
    code += function(game, name)
for name in ('Sram_SetFlashPagesDefault','Sram_StartWriteToFlashDefault','Sram_UpdateWriteToFlashDefault',
             'Sram_SetFlashPagesOwlSave','Sram_StartWriteToFlashOwlSave','Sram_UpdateWriteToFlashOwlSave'):
    code += function(sram, name)
for name in ('FileSelect_NameEntryWaitForFlashSave','FileSelect_OptionsWaitForFlashSave'):
    code += function(names, name)
code += r'''
static void check_create(int options) {
    u8 data[SAVE_BUFFER_SIZE],readback[SAVE_BUFFER_SIZE];
    memset(data,options ? 0x6b : 0x39,sizeof(data));
    FileSelectState file={.sramCtx={.saveBuf=data},.configMode=options ? CM_OPTIONS_WAIT_FOR_FLASH_SAVE : CM_NAME_ENTRY_WAIT_FOR_FLASH_SAVE};
    Sram_SetFlashPagesDefault(&file.sramCtx,options ? 0x380 : 0,options ? 1 : 0x80);
    Sram_StartWriteToFlashDefault(&file.sramCtx);
    assert(SysFlashrom_IsBusy()==1); // A completed request is ready to collect.
    if (options) FileSelect_OptionsWaitForFlashSave(&file); else FileSelect_NameEntryWaitForFlashSave(&file);
    assert(file.sramCtx.status==4 && SysFlashrom_IsBusy()==0);
    nowUsec+=1999999;
    if (options) FileSelect_OptionsWaitForFlashSave(&file); else FileSelect_NameEntryWaitForFlashSave(&file);
    assert(file.sramCtx.status==4);
    nowUsec++;
    if (options) FileSelect_OptionsWaitForFlashSave(&file); else FileSelect_NameEntryWaitForFlashSave(&file);
    assert(file.sramCtx.status==0 && file.configMode==(options ? CM_OPTIONS_TO_MAIN : CM_NAME_ENTRY_TO_MAIN));
    memset(sMmPspFlash,0,sizeof(sMmPspFlash));MmPsp_LoadFlash();
    assert(SysFlashrom_Read(readback,file.sramCtx.curPage,file.sramCtx.numPages)==0);
    assert(memcmp(readback,data,file.sramCtx.numPages*FLASH_BLOCK_SIZE)==0);
}
int main(int argc,char** argv) {
    assert(argc==1 || argc==2);
    if (argc==2) {
        OSTime start=osGetTime();nowUsec+=2000000;
        assert(osGetTime()-start==93750000ULL);
        assert(OSTIME_TO_TIMER(osGetTime()-start)==200);
        puts("PSP clock: N64 cycles and two-second save deadline passed");return 0;
    }
    assert(SysFlashrom_InitFlash()==0 && SysFlashrom_IsInit());
    assert(SysFlashrom_IsBusy()==0);
    check_create(0);check_create(1);
    u8 data[SAVE_BUFFER_SIZE],other[SAVE_BUFFER_SIZE];
    memset(data,0x7d,sizeof(data));
    SramContext owl={.saveBuf=data};
    Sram_SetFlashPagesOwlSave(&owl,0x100,0x80);Sram_StartWriteToFlashOwlSave(&owl);
    Sram_UpdateWriteToFlashOwlSave(&owl);assert(owl.status==8 && SysFlashrom_IsBusy()==1);
    Sram_UpdateWriteToFlashOwlSave(&owl);assert(owl.status==4 && SysFlashrom_IsBusy()==0);
    assert(SysFlashrom_Read(other,0x100,0x80)==0 && memcmp(other,data,sizeof(data))==0);
    assert(SysFlashrom_Read(other,0x180,0x80)==0 && memcmp(other,data,sizeof(data))==0);
    nowUsec+=2000000;Sram_UpdateWriteToFlashOwlSave(&owl);assert(owl.status==0);
    // Invalid ranges and I/O failures still deliver a result, avoiding a stuck wait state.
    SysFlashrom_WriteAsync(data,0x400,1);assert(SysFlashrom_IsBusy()==1);
    assert(SysFlashrom_AwaitResult()==-1 && SysFlashrom_IsBusy()==0);
    SysFlashrom_WriteAsync(NULL,0,1);assert(SysFlashrom_IsBusy()==1);
    assert(SysFlashrom_AwaitResult()==-1 && SysFlashrom_IsBusy()==0);
    failOpen=1;SysFlashrom_WriteAsync(data,0,1);assert(SysFlashrom_IsBusy()==1);
    assert(SysFlashrom_AwaitResult()==-1 && SysFlashrom_IsBusy()==0);failOpen=0;
    failWrite=1;SysFlashrom_WriteAsync(data,0,1);assert(SysFlashrom_IsBusy()==1);
    assert(SysFlashrom_AwaitResult()==-1 && SysFlashrom_IsBusy()==0);failWrite=0;
    SysFlashrom_WriteSync(data,0,1);assert(SysFlashrom_IsBusy()==0);
    assert(SysFlashrom_Read(other,0,1)==0 && memcmp(other,data,FLASH_BLOCK_SIZE)==0);
    // Atomic commit & recovery: failed partial writes, close failures, rename/remove
    // failures and mid-commit crashes preserve prior valid generation on disk & RAM.
    u8 newData[SAVE_BUFFER_SIZE],baselineData[FLASH_BLOCK_SIZE];
    memcpy(baselineData,data,FLASH_BLOCK_SIZE);
    memset(newData,0xA5,sizeof(newData));
    writeBudget=65536;
    SysFlashrom_WriteAsync(newData,0,1);
    assert(SysFlashrom_AwaitResult()==-1); writeBudget=-1;
    assert(SysFlashrom_Read(other,0,1)==0 && memcmp(other,baselineData,FLASH_BLOCK_SIZE)==0);
    memset(sMmPspFlash,0,sizeof(sMmPspFlash)); MmPsp_LoadFlash();
    assert(SysFlashrom_Read(other,0,1)==0 && memcmp(other,baselineData,FLASH_BLOCK_SIZE)==0);

    zeroWriteOnce=1;
    SysFlashrom_WriteAsync(newData,0,1);
    assert(SysFlashrom_AwaitResult()==-1);
    memset(sMmPspFlash,0,sizeof(sMmPspFlash)); MmPsp_LoadFlash();
    assert(SysFlashrom_Read(other,0,1)==0 && memcmp(other,baselineData,FLASH_BLOCK_SIZE)==0);

    failClose=1;
    SysFlashrom_WriteAsync(newData,0,1);
    assert(SysFlashrom_AwaitResult()==-1);
    memset(sMmPspFlash,0,sizeof(sMmPspFlash)); MmPsp_LoadFlash();
    assert(SysFlashrom_Read(other,0,1)==0 && memcmp(other,baselineData,FLASH_BLOCK_SIZE)==0);

    failRemove=1;
    SysFlashrom_WriteAsync(newData,0,1);
    assert(SysFlashrom_AwaitResult()==-1); failRemove=0;
    memset(sMmPspFlash,0,sizeof(sMmPspFlash)); MmPsp_LoadFlash();
    assert(SysFlashrom_Read(other,0,1)==0 && memcmp(other,baselineData,FLASH_BLOCK_SIZE)==0);

    renameCount=0; failRenameNum=1;
    SysFlashrom_WriteAsync(newData,0,1);
    assert(SysFlashrom_AwaitResult()==-1); failRenameNum=0;
    memset(sMmPspFlash,0,sizeof(sMmPspFlash)); MmPsp_LoadFlash();
    assert(SysFlashrom_Read(other,0,1)==0 && memcmp(other,baselineData,FLASH_BLOCK_SIZE)==0);

    renameCount=0; failRenameNum=2;
    SysFlashrom_WriteAsync(newData,0,1);
    assert(SysFlashrom_AwaitResult()==-1); failRenameNum=0;
    memset(sMmPspFlash,0,sizeof(sMmPspFlash)); MmPsp_LoadFlash();
    assert(SysFlashrom_Read(other,0,1)==0 && memcmp(other,baselineData,FLASH_BLOCK_SIZE)==0);

    // Ensure steady-state backup exists (generation N live + N-1 .bak), then simulate
    // interrupted/corrupted live file and deleted live file recovering cleanly from .bak.
    SysFlashrom_WriteSync(baselineData,0,1);
    FILE* f = fopen(MM_PSP_SAVE_PATH, "wb");
    assert(f != NULL);
    fwrite(newData, 1, 1024, f);
    fclose(f);
    memset(sMmPspFlash,0,sizeof(sMmPspFlash)); MmPsp_LoadFlash();
    assert(SysFlashrom_Read(other,0,1)==0 && memcmp(other,baselineData,FLASH_BLOCK_SIZE)==0);

    assert(remove(MM_PSP_SAVE_PATH) == 0);
    memset(sMmPspFlash,0,sizeof(sMmPspFlash)); MmPsp_LoadFlash();
    assert(SysFlashrom_Read(other,0,1)==0 && memcmp(other,baselineData,FLASH_BLOCK_SIZE)==0);

    // Legacy raw 128 KiB save without .bak sidecar loads cleanly.
    remove(MM_PSP_SAVE_PATH ".bak");
    memset(sMmPspFlash,0,sizeof(sMmPspFlash)); MmPsp_LoadFlash();
    assert(SysFlashrom_Read(other,0,1)==0 && memcmp(other,baselineData,FLASH_BLOCK_SIZE)==0);

    puts("Flash saves: create/options exit Please Wait after two seconds, persistence, owl backups, atomic commit, sync and failure recovery passed");
}
'''
with tempfile.TemporaryDirectory(prefix='mm-flash-save-') as directory:
    path = Path(directory)
    (path / 'test.c').write_text(code)
    subprocess.run([os.environ.get('CC','cc'),'-std=c11','-O2','-Wall','-Wextra','-Werror',
                    '-Wno-unused-parameter','-I',str(ROOT / 'include'),str(path/'test.c'),
                    '-o',str(path/'test')],check=True)
    # Run separately so both missing completion and incorrect clock units are caught.
    results = [subprocess.run([str(path/'test'),*args],cwd=path).returncode for args in (['clock'],[])]
    if any(results):
        raise SystemExit(1)
