/* somtum-pp C ABI. Every struct is f64-only; field order is the ABI (append only). */
#ifndef SOMTUM_PP_H
#define SOMTUM_PP_H
#include <stddef.h>
#include <stdint.h>
#ifdef __cplusplus
extern "C" {
#endif

typedef struct SppMap SppMap;

/* Unset optional fields are -1: clock_rate, max_combo, large_tick_hits, slider_end_hits,
 * passed_objects, legacy_total_score, accuracy. accuracy >= 0 generates hits from it. */
typedef struct SppScore {
    double mode, mods, lazer, clock_rate, max_combo;
    double n300, n100, n50, n_geki, n_katu, n_miss;
    double large_tick_hits, slider_end_hits, passed_objects, legacy_total_score, accuracy;
} SppScore;

/* calculator: 0 official (ppy), 1 relax (akatsuki osu_2019), 2 akatsuki main; -1 failed. */
typedef struct SppPerf {
    double pp, pp_aim, pp_speed, pp_acc, pp_flashlight, stars, max_combo, effective_miss_count, calculator;
} SppPerf;

typedef struct SppStats {
    double ar, od, cs, hp, clock_rate, n_objects, mode;
} SppStats;

SppMap *spp_map_new(const uint8_t *bytes, size_t len);
void spp_map_free(SppMap *map);
int32_t spp_map_mode(const SppMap *map);
uint32_t spp_map_n_objects(const SppMap *map);
int32_t spp_calculate(SppMap *map, const SppScore *score, SppPerf *out);
int32_t spp_stats(const SppMap *map, const SppScore *score, SppStats *out);
int64_t spp_strains(const SppMap *map, const SppScore *score, double *out, size_t cap, double *section_len_ms);
int32_t spp_route(uint32_t mode, uint32_t mods);
const char *spp_version(void);
uint32_t spp_abi_score_fields(void);

#ifdef __cplusplus
}
#endif
#endif
