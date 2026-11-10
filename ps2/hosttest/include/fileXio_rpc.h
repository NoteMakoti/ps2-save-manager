#pragma once
static inline int fileXioOpen(const char *p, int f, ...) { (void)p; (void)f; return -1; }
static inline int fileXioRead(int fd, void *b, int n) { (void)fd; (void)b; (void)n; return -1; }
static inline int fileXioClose(int fd) { (void)fd; return 0; }
