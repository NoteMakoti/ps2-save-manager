#pragma once
#include <unistd.h>
static inline int DelayThread(int us) { return usleep(us); }
