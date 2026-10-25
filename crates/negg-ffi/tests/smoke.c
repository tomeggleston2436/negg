/* SPDX-License-Identifier: Apache-2.0 */
/* C smoke test for the negg C ABI. Usage: smoke ROLL_DIR OUT_DIR */
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "negg.h"

#define CHECK(cond, what)                                      \
    do {                                                       \
        if (!(cond)) {                                         \
            char msg[512];                                     \
            negg_last_error(msg, sizeof msg);                  \
            fprintf(stderr, "FAIL %s (line %d): %s\n", what, __LINE__, msg); \
            return 1;                                          \
        }                                                      \
    } while (0)

int main(int argc, char **argv) {
    if (argc < 3) {
        fprintf(stderr, "usage: smoke ROLL_DIR OUT_DIR\n");
        return 2;
    }
    CHECK(negg_abi_version() == NEGG_ABI_VERSION, "abi version");

    NeggSession *s = NULL;
    CHECK(negg_open_roll(NULL, &s) == NeggStatus_Null, "NULL dir");
    CHECK(negg_open_roll(argv[1], &s) == NeggStatus_Ok, "open roll");
    CHECK(s != NULL, "handle");

    uint32_t n = 0;
    CHECK(negg_frame_count(s, &n) == NeggStatus_Ok, "frame count");
    CHECK(n > 0, "frames");

    /* A too-small buffer reports the size needed; then a big enough one works. */
    uint8_t small[8];
    size_t need = 0;
    CHECK(negg_render_tile(s, 0, 0, 0, 0, small, sizeof small, &need) == NeggStatus_BufferTooSmall, "small buffer");
    CHECK(need > 48, "tile size");
    uint8_t *tile = malloc(need);
    size_t got = 0;
    CHECK(negg_render_tile(s, 0, 0, 0, 0, tile, need, &got) == NeggStatus_Ok, "tile");
    CHECK(got == need, "tile length");
    CHECK(memcmp(tile, "NEGGTILE", 8) == 0, "tile magic");
    free(tile);

    char *json = NULL;
    CHECK(negg_get_stats(s, 0, &json) == NeggStatus_Ok, "stats");
    CHECK(json != NULL && strstr(json, "\"estimates\"") != NULL, "stats json");
    negg_string_free(json);

    CHECK(negg_set_settings(s, 0, "{not json") == NeggStatus_Json, "bad json");
    {
        size_t want = negg_last_error(NULL, 0);
        CHECK(want > 1, "last error size");
        char *msg = malloc(want);
        CHECK(negg_last_error(msg, want) == want, "last error copy");
        CHECK(strlen(msg) == want - 1 && strstr(msg, "JSON") != NULL, "last error text");
        free(msg);
    }
    CHECK(negg_set_roll_settings(s, "[1,2") == NeggStatus_Json, "bad roll json");
    CHECK(negg_set_settings(s, 0, "{}") == NeggStatus_Ok, "set settings");
    CHECK(negg_get_stats(s, n, &json) == NeggStatus_NotFound, "no such frame");

    CHECK(negg_set_roll_settings(s, "{}") == NeggStatus_Ok, "set roll settings");
    char *report = NULL;
    CHECK(negg_export(s, 0, 1, argv[2], &report) == NeggStatus_Ok, "export");
    CHECK(report != NULL && strstr(report, "\"converted\":1") != NULL, "report");
    negg_string_free(report);

    negg_session_free(s);
    negg_session_free(NULL);
    printf("smoke ok\n");
    return 0;
}
