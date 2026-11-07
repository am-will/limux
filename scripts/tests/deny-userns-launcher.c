#define _GNU_SOURCE

#include <errno.h>
#include <linux/filter.h>
#include <linux/sched.h>
#include <linux/seccomp.h>
#include <stddef.h>
#include <stdio.h>
#include <stdlib.h>
#include <sys/prctl.h>
#include <sys/syscall.h>
#include <unistd.h>

#define LOAD_SYSCALL \
  BPF_STMT(BPF_LD | BPF_W | BPF_ABS, offsetof(struct seccomp_data, nr))
#define DENY_FLAGS_SYSCALL(number)                                      \
  LOAD_SYSCALL,                                                         \
      BPF_JUMP(BPF_JMP | BPF_JEQ | BPF_K, number, 0, 4),                \
      BPF_STMT(BPF_LD | BPF_W | BPF_ABS,                                \
               offsetof(struct seccomp_data, args[0])),                 \
      BPF_JUMP(BPF_JMP | BPF_JSET | BPF_K, CLONE_NEWUSER, 0, 1),        \
      BPF_STMT(BPF_RET | BPF_K, SECCOMP_RET_ERRNO | (EPERM & SECCOMP_RET_DATA)), \
      BPF_STMT(BPF_RET | BPF_K, SECCOMP_RET_ALLOW)

int main(int argc, char **argv) {
  if (argc < 2) {
    fprintf(stderr, "usage: deny-userns-launcher command [args...]\n");
    return 2;
  }

  struct sock_filter filter[] = {
      DENY_FLAGS_SYSCALL(SYS_clone),
      DENY_FLAGS_SYSCALL(SYS_unshare),
      LOAD_SYSCALL,
      BPF_JUMP(BPF_JMP | BPF_JEQ | BPF_K, SYS_clone3, 0, 1),
      BPF_STMT(BPF_RET | BPF_K, SECCOMP_RET_ERRNO | (ENOSYS & SECCOMP_RET_DATA)),
      BPF_STMT(BPF_RET | BPF_K, SECCOMP_RET_ALLOW),
  };
  struct sock_fprog program = {
      .len = (unsigned short)(sizeof(filter) / sizeof(filter[0])),
      .filter = filter,
  };

  if (prctl(PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) != 0 ||
      prctl(PR_SET_SECCOMP, SECCOMP_MODE_FILTER, &program) != 0) {
    perror("installing test user-namespace seccomp filter");
    return 125;
  }

  execvp(argv[1], &argv[1]);
  perror("execvp");
  return 127;
}
