// Read-only development diagnostic, never linked into the product.
// Coalition metadata ABI: Apple XNU xnu-11417.140.69, proc_info_private.h
// (flavor 20, 40 bytes) and coalition.h (resource type 0, usage prefix).
// Requires an exact app executable path. No task ports, process contents,
// arguments, UI access, privilege changes or coalition mutations.
#include <dlfcn.h>
#include <errno.h>
#include <inttypes.h>
#include <libproc.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/proc_info.h>
#include <sys/resource.h>
#include <unistd.h>

enum { MAX_PIDS = 32768, MAX_MEMBERS = 64, PID_COALITION = 20 };
struct coalition_ids { uint64_t resource, jetsam, reserved[3]; };
// Only request the stable leading two counters; no hand-written full usage
// layout or platform-dependent footprint offset is needed.
struct counts { uint64_t started, exited; };
typedef int (*coalition_usage_fn)(uint64_t, void *, size_t);
_Static_assert(sizeof(struct coalition_ids) == 40, "coalition ABI size");
_Static_assert(sizeof(struct counts) == 16, "coalition usage prefix size");

struct member {
  pid_t pid;
  struct proc_bsdinfo identity;
  struct rusage_info_v4 usage;
  char path[PROC_PIDPATHINFO_MAXSIZE];
};
struct census { struct member members[MAX_MEMBERS]; int count, unreadable_ids; };

static bool identity(pid_t pid, struct proc_bsdinfo *result) {
  return proc_pidinfo(pid, PROC_PIDTBSDINFO, 0, result, sizeof(*result)) == sizeof(*result)
      && result->pbi_pid == (uint32_t)pid;
}
static bool same(const struct proc_bsdinfo *a, const struct proc_bsdinfo *b) {
  return a->pbi_pid == b->pbi_pid && a->pbi_ppid == b->pbi_ppid
      && a->pbi_start_tvsec == b->pbi_start_tvsec
      && a->pbi_start_tvusec == b->pbi_start_tvusec;
}
static bool coalitions(pid_t pid, struct coalition_ids *result) {
  memset(result, 0, sizeof(*result));
  return proc_pidinfo(pid, PID_COALITION, 0, result, sizeof(*result)) == sizeof(*result)
      && !result->reserved[0] && !result->reserved[1] && !result->reserved[2];
}
static int pids(pid_t *result) {
  int bytes = proc_listpids(PROC_ALL_PIDS, 0, result, MAX_PIDS * sizeof(*result));
  if (bytes <= 0 || bytes >= (int)(MAX_PIDS * sizeof(*result)) || bytes % sizeof(*result)) return -1;
  return bytes / (int)sizeof(*result);
}
static int compare(const void *a, const void *b) {
  pid_t x = ((const struct member *)a)->pid, y = ((const struct member *)b)->pid;
  return (x > y) - (x < y);
}
static bool collect(uint64_t coalition, struct census *out) {
  pid_t all[MAX_PIDS];
  int count = pids(all);
  if (count < 0) return false;
  for (int i = 0; i < count; ++i) {
    if (all[i] <= 0) continue;
    struct coalition_ids ids;
    if (!coalitions(all[i], &ids)) { ++out->unreadable_ids; continue; }
    if (ids.resource != coalition) continue;
    if (out->count == MAX_MEMBERS) return false;
    struct member *m = &out->members[out->count];
    m->pid = all[i];
    struct proc_bsdinfo after;
    if (!identity(m->pid, &m->identity)
        || proc_pidpath(m->pid, m->path, sizeof(m->path)) <= 0
        || proc_pid_rusage(m->pid, RUSAGE_INFO_V4, (rusage_info_t *)&m->usage)
        || !identity(m->pid, &after) || !same(&m->identity, &after)
        || !coalitions(m->pid, &ids) || ids.resource != coalition) return false;
    ++out->count;
  }
  qsort(out->members, (size_t)out->count, sizeof(out->members[0]), compare);
  return true;
}
static void string(const char *value) {
  putchar('"');
  for (const unsigned char *p = (const unsigned char *)value; *p; ++p) {
    if (*p == '"' || *p == '\\') printf("\\%c", *p);
    else if (*p < 32) printf("\\u%04x", *p);
    else putchar(*p);
  }
  putchar('"');
}
static int fail(const char *reason) {
  printf("{\"complete\":false,\"reason\":"); string(reason); puts("}");
  return 1;
}
int main(int argc, char **argv) {
  if (argc < 2 || argc > 3 || argv[1][0] != '/' || !strstr(argv[1], ".app/Contents/MacOS/")) return 64;
  uint64_t expected = 0;
  if (argc == 3) {
    char *end = NULL;
    errno = 0;
    expected = strtoull(argv[2], &end, 10);
    if (errno || !end || *end || !expected || argv[2][0] == '-') return 64;
  }
  coalition_usage_fn usage = (coalition_usage_fn)dlsym(RTLD_DEFAULT, "coalition_info_resource_usage");
  if (!usage) return fail("coalition_usage_unavailable");
  pid_t all[MAX_PIDS], root = 0;
  int count = pids(all);
  if (count < 0) return fail("pid_list_unavailable");
  for (int i = 0; i < count; ++i) {
    char path[PROC_PIDPATHINFO_MAXSIZE] = {0};
    if (all[i] > 0 && proc_pidpath(all[i], path, sizeof(path)) > 0 && !strcmp(path, argv[1])) {
      if (root) return fail("multiple_matching_apps");
      root = all[i];
    }
  }
  if (!root) return fail("app_not_running");
  struct proc_bsdinfo root_before, root_after;
  struct coalition_ids ids, self_ids, init_ids, after_ids;
  struct rusage_info_v4 root_usage = {0};
  if (!identity(root, &root_before) || root_before.pbi_uid != getuid()
      || !coalitions(root, &ids) || !coalitions(getpid(), &self_ids) || !coalitions(1, &init_ids)
      || proc_pid_rusage(root, RUSAGE_INFO_V4, (rusage_info_t *)&root_usage)) return fail("root_identity_unavailable");
  if (expected && expected != root_usage.ri_proc_start_abstime) return fail("root_identity_changed");
  if (!ids.resource || ids.resource == self_ids.resource || ids.resource == init_ids.resource)
    return fail("app_coalition_not_isolated");
  struct counts begin = {0}, end = {0};
  struct census first = {0}, second = {0};
  bool valid = !usage(ids.resource, &begin, sizeof(begin)) && collect(ids.resource, &first)
      && collect(ids.resource, &second) && !usage(ids.resource, &end, sizeof(end))
      && begin.started == end.started && begin.exited == end.exited && begin.started >= begin.exited
      && begin.started - begin.exited == (uint64_t)first.count && first.count == second.count
      && identity(root, &root_after) && same(&root_before, &root_after)
      && coalitions(root, &after_ids) && ids.resource == after_ids.resource;
  bool found_root = false;
  for (int i = 0; i < first.count; ++i) {
    const struct member *a = &first.members[i], *b = &second.members[i];
    valid = valid && same(&a->identity, &b->identity)
        && a->usage.ri_proc_start_abstime == b->usage.ri_proc_start_abstime && !strcmp(a->path, b->path);
    if (a->pid == root && same(&a->identity, &root_before)
        && a->usage.ri_proc_start_abstime == root_usage.ri_proc_start_abstime && !strcmp(a->path, argv[1])) found_root = true;
  }
  valid = valid && found_root;
  printf("{\"complete\":%s,\"reason\":%s,\"root_pid\":%d,\"root_started\":%" PRIu64 ",\"resource_coalition\":%" PRIu64 ",",
      valid ? "true" : "false", valid ? "null" : "\"membership_or_identity_changed\"", root, root_usage.ri_proc_start_abstime, ids.resource);
  printf("\"tasks_started_before\":%" PRIu64 ",\"tasks_exited_before\":%" PRIu64 ",\"tasks_started_after\":%" PRIu64 ",\"tasks_exited_after\":%" PRIu64 ",",
      begin.started, begin.exited, end.started, end.exited);
  printf("\"unreadable_coalition_ids_before\":%d,\"unreadable_coalition_ids_after\":%d,\"processes\":[", first.unreadable_ids, second.unreadable_ids);
  for (int i = 0; i < first.count; ++i) {
    const struct member *m = &first.members[i];
    printf("%s{\"pid\":%d,\"parent\":%u,\"started\":%" PRIu64 ",\"path\":", i ? "," : "", m->pid, m->identity.pbi_ppid, m->usage.ri_proc_start_abstime);
    string(m->path);
    printf(",\"rss_bytes\":%" PRIu64 ",\"footprint_bytes\":%" PRIu64 ",\"peak_footprint_bytes\":%" PRIu64 "}", m->usage.ri_resident_size, m->usage.ri_phys_footprint, m->usage.ri_lifetime_max_phys_footprint);
  }
  puts("]}");
  return valid ? 0 : 1;
}
