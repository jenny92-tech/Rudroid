// INPUT:  public Linux ARM64 GNU epoll/pthread/time headers
// OUTPUT:  compile-time native ABI checks; no runtime functions or data
// POS:    attests the native types used by Rust's single event owner
#include <stddef.h>
#include <stdint.h>
#include <pthread.h>
#include <sys/epoll.h>
#include <time.h>
_Static_assert(sizeof(void *)==8 && sizeof(pthread_key_t)==4,"ARM64 GNU LP64/TLS key");
_Static_assert(sizeof(struct epoll_event)==16 && _Alignof(struct epoll_event)==8 &&
    offsetof(struct epoll_event,events)==0 && offsetof(struct epoll_event,data)==8,"epoll ABI");
_Static_assert(sizeof(struct timespec)==16 && offsetof(struct timespec,tv_nsec)==8,"time ABI");
