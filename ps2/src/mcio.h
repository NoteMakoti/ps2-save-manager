#ifndef PS2SM_MCIO_H
#define PS2SM_MCIO_H

#include <libmc.h>
#include "common.h"

#define MCIO_MAX_ENTRIES 256

/* Errors of our own (libmc results are > -100). */
#define MCIO_E_TIMEOUT -2000   /* command did not finish in time */
#define MCIO_E_BUSY -2001      /* an earlier command is still running */
#define MCIO_E_NOT_READY -2002 /* libmc not initialised / nothing queued */
#define MCIO_E_BAD_PSU -2003
#define MCIO_E_EXISTS -2004
#define MCIO_E_STORAGE -2005  /* USB/SMB backup file could not be written */
#define MCIO_IS_DRIVER_ERROR(r) ((r) == MCIO_E_TIMEOUT || (r) == MCIO_E_BUSY || (r) == MCIO_E_NOT_READY)

typedef struct
{
    int present;   /* 1 if a card is inserted */
    int type;      /* sceMcType* */
    int formatted; /* 1 if formatted */
    int free_kb;
} mc_info_t;

typedef struct
{
    char name[33];
    char title[72];      /* ASCII folded title for display */
    u8 title_sjis[72];   /* raw title, two lines joined by a space */
    int title_sjis_len;
    u32 size;            /* total bytes of files */
    u16 mode;
    u8 created[8];
    u8 modified[8];
} save_info_t;

int mcio_init(void);
/* 1 if libmc initialised and no command is stuck */
int mcio_ready(void);
const char *mcio_strerror(int r);
int mcio_set_info(int port, const char *path, const sceMcTblGetDir *info);
int mcio_info(int port, mc_info_t *info);

/* Directory listing; path like "/" or "/BESLES-12345". Excludes . and .. */
int mcio_list(int port, const char *path, sceMcTblGetDir **out, int *count);

int mcio_read_file(int port, const char *path, u8 **data, int *len);
int mcio_write_file(int port, const char *path, const u8 *data, int len);
int mcio_mkdir(int port, const char *path);
int mcio_delete(int port, const char *path);
int mcio_delete_tree(int port, const char *dir);
int mcio_format(int port);

/* All top-level saves with titles. Caller frees *out. */
int mcio_list_saves(int port, save_info_t **out, int *count);

/* Save <-> .psu image */
int mcio_save_to_psu(int port, const char *dir, buf_t *out);
int mcio_psu_to_card(int port, const u8 *psu, int len, int overwrite, char *dirname_out);
int psu_get_dirname(const u8 *psu, int len, char *out);

/* Maps libmc result codes to protocol status codes. */
int mcio_status(int mcres);

#endif
