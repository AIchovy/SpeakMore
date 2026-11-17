// Read-only development sampler. Enumerates only descendants of an explicit
// test PID. SDK structs and libproc APIs; no UI automation or task port access.
#include <libproc.h>
#include <sys/proc_info.h>
#include <sys/resource.h>
#include <errno.h>
#include <stdio.h>
#include <stdlib.h>

static int visited = 0;
static int sample(int pid, int parent, int depth) {
  if (++visited > 64 || depth > 8) return 1;
  struct proc_bsdinfo before = {0}, after = {0};
  struct rusage_info_v4 usage = {0};
  if (proc_pidinfo(pid, PROC_PIDTBSDINFO, 0, &before, sizeof(before)) != sizeof(before)
      || (parent && before.pbi_ppid != (unsigned)parent)
      || proc_pid_rusage(pid, RUSAGE_INFO_V4, (rusage_info_t *)&usage) != 0
      || proc_pidinfo(pid, PROC_PIDTBSDINFO, 0, &after, sizeof(after)) != sizeof(after)
      || before.pbi_start_tvsec != after.pbi_start_tvsec
      || before.pbi_start_tvusec != after.pbi_start_tvusec
      || before.pbi_ppid != after.pbi_ppid) return 1;
  printf("{\"pid\":%d,\"parent\":%d,\"depth\":%d,\"started\":%llu,\"rss_bytes\":%llu,\"footprint_bytes\":%llu,\"peak_footprint_bytes\":%llu}\n",
      pid, parent, depth, (unsigned long long)usage.ri_proc_start_abstime,
      (unsigned long long)usage.ri_resident_size,
      (unsigned long long)usage.ri_phys_footprint,
      (unsigned long long)usage.ri_lifetime_max_phys_footprint);
  int children[64] = {0};
  errno = 0;
  // Unlike proc_listpids, this convenience wrapper returns a PID count.
  int count = proc_listchildpids(pid, children, sizeof(children));
  if (count < 0 || count >= 64 || errno) return 1;
  for (int i = 0; i < count; ++i) {
    if (children[i] > 0 && sample(children[i], pid, depth + 1)) return 1;
  }
  return 0;
}
int main(int argc, char **argv) {
  if (argc != 2) return 64;
  char *end = NULL;
  errno = 0;
  long pid = strtol(argv[1], &end, 10);
  if (errno || !end || *end || pid < 1 || pid > 2147483647) return 64;
  return sample((int)pid, 0, 0);
}
