#include <stdint.h>
#include <stdbool.h>
#include <string.h>
#ifdef _WIN32
#include <windows.h>
#define API __declspec(dllexport)
#else
#include <time.h>
#define API __attribute__((visibility("default")))
#endif

API int32_t answer(void) { return 42; }
API int8_t negate8(int8_t x) { return (int8_t)-x; }
API uint8_t echo8(uint8_t x) { return x; }
API int16_t echo16(int16_t x) { return x; }
API uint16_t echou16(uint16_t x) { return x; }
API int32_t echo32(int32_t x) { return x; }
API uint32_t echou32(uint32_t x) { return x; }
API int64_t echo64(int64_t x) { return x; }
API uint64_t echou64(uint64_t x) { return x; }
API bool invert(bool x) { return !x; }
API int64_t many_ints(int64_t a, int64_t b, int64_t c, int64_t d, int64_t e,
                     int64_t f, int64_t g, int64_t h, int64_t i) {
    return a + b + c + d + e + f + g + h + i;
}
API double many_floats(double a, double b, double c, double d, double e,
                       double f, double g, double h, double i, double j) {
    return a + b + c + d + e + f + g + h + i + j;
}
API double mixed(int8_t a, double b, uint32_t c, double d, int64_t e,
                 double f, int32_t g, double h, int16_t i, double j) {
    return a + b + c + d + e + f + g + h + i + j;
}
API uint64_t length(const char *value) { return (uint64_t)strlen(value); }
API void fill(uint8_t *out, uint64_t count, uint8_t value) { memset(out, value, (size_t)count); }
API const char *greeting(void) { return "native"; }
API void pause_ms(int32_t ms) {
#ifdef _WIN32
    Sleep((DWORD)ms);
#else
    struct timespec delay = { ms / 1000, (ms % 1000) * 1000000L };
    nanosleep(&delay, 0);
#endif
}
