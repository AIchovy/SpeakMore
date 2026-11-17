// Developer-only read-only sampler. Uses SDK structures, not handwritten layouts.
#include <libproc.h>
#include <sys/resource.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <errno.h>
int main(int argc, char **argv) {
  if (argc < 2) return 64;
  for (int i = 1; i < argc; ++i) {
    char *end = NULL;
    errno = 0; long value = strtol(argv[i], &end, 10);
    if (errno || !end || *end || value < 1 || value > 2147483647) return 64;
    struct rusage_info_v4 r = {0};
    if (proc_pid_rusage((int)value, RUSAGE_INFO_V4, (rusage_info_t *)&r) != 0) {
      fprintf(stderr, "ProcessMetricsUnavailable\n"); return 1;
    }
    printf("{\"pid\":%ld,\"started\":%llu,\"rss_bytes\":%llu,\"footprint_bytes\":%llu,\"peak_footprint_bytes\":%llu}\n", value,
      (unsigned long long)r.ri_proc_start_abstime, (unsigned long long)r.ri_resident_size,
      (unsigned long long)r.ri_phys_footprint, (unsigned long long)r.ri_lifetime_max_phys_footprint);
  }
  return 0;
}
