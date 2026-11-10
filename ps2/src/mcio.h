#ifndef PS2SM_MCIO_H
#define PS2SM_MCIO_H

#include <libmc.h>
#include "common.h"

#define MCIO_MAX_ENTRIES 256

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
