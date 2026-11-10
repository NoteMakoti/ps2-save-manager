/* host shim for libmc: a directory on disk plays the memory card */
#pragma once
#include <tamtypes.h>
typedef struct { u8 Resv2, Sec, Min, Hour, Day, Month; u16 Year; } sceMcStDateTime;
typedef struct {
    sceMcStDateTime _Create, _Modify;
    u32 FileSizeByte; u16 AttrFile; u16 Reserve1; u32 Reserve2; u32 PdaAplNo;
    unsigned char EntryName[32];
} sceMcTblGetDir;
#define sceMcResSucceed 0
#define sceMcResChangedCard -1
#define sceMcResNoFormat -2
#define sceMcResFullDevice -3
#define sceMcResNoEntry -4
#define sceMcResDeniedPermit -5
#define sceMcResNotEmpty -6
#define sceMcResFailDetect -12
#define sceMcTypeNoCard 0
#define sceMcTypePS1 1
#define sceMcTypePS2 2
#define MC_TYPE_XMC 1
int mcInit(int); int mcSync(int, int*, int*);
int mcGetInfo(int, int, int*, int*, int*);
int mcOpen(int, int, const char*, int); int mcClose(int); int mcSeek(int, int, int);
int mcRead(int, void*, int); int mcWrite(int, const void*, int);
int mcMkDir(int, int, const char*); int mcDelete(int, int, const char*);
int mcGetDir(int, int, const char*, unsigned, int, sceMcTblGetDir*);
int mcSetFileInfo(int, int, const char*, const sceMcTblGetDir*, unsigned);
int mcFormat(int, int);
