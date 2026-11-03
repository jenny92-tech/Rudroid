// INPUT:  Android ARM64 socket/iovec/addrinfo contracts and native POSIX APIs
// OUTPUT:  rudroid_bionic_network_symbol/_symbol_name table, native vectored I/O/socket addresses and translated addrinfo
// POS:    shares native descriptors and DNS data with one C-owned result facade allocation
#define _GNU_SOURCE
#include <arpa/inet.h>
#include <errno.h>
#include <limits.h>
#include <netdb.h>
#include <net/if.h>
#include <stddef.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/select.h>
#include <sys/socket.h>
#include <sys/uio.h>
#include <unistd.h>
#include "symbol_table.h"

typedef struct GuestAddrInfo {
    int flags,family,type,protocol; unsigned length;
    char *canonical; struct sockaddr *address; struct GuestAddrInfo *next;
} GuestAddrInfo;
typedef struct { char *h_name; char **h_aliases; int h_addrtype,h_length; char **h_addr_list; } GuestHostent;
_Static_assert(sizeof(GuestHostent)==sizeof(struct hostent) &&
    offsetof(GuestHostent,h_name)==offsetof(struct hostent,h_name) &&
    offsetof(GuestHostent,h_aliases)==offsetof(struct hostent,h_aliases) &&
    offsetof(GuestHostent,h_addrtype)==offsetof(struct hostent,h_addrtype) &&
    offsetof(GuestHostent,h_length)==offsetof(struct hostent,h_length) &&
    offsetof(GuestHostent,h_addr_list)==offsetof(struct hostent,h_addr_list), "hostent public ABI");
typedef struct { struct addrinfo *native; GuestAddrInfo nodes[]; } Results;
_Static_assert(sizeof(GuestAddrInfo)==48 && offsetof(GuestAddrInfo,canonical)==24 &&
    offsetof(GuestAddrInfo,address)==32 && offsetof(GuestAddrInfo,next)==40, "Android addrinfo ABI");
_Static_assert(sizeof(struct iovec)==16 && offsetof(struct iovec,iov_len)==8 &&
    sizeof(struct msghdr)==56 && offsetof(struct msghdr,msg_name)==0 &&
    offsetof(struct msghdr,msg_namelen)==8 && offsetof(struct msghdr,msg_iov)==16 &&
    offsetof(struct msghdr,msg_iovlen)==24 && offsetof(struct msghdr,msg_control)==32 &&
    offsetof(struct msghdr,msg_controllen)==40 && offsetof(struct msghdr,msg_flags)==48 &&
    sizeof(struct sockaddr_in)==16 && sizeof(struct sockaddr_in6)==28 &&
    sizeof(socklen_t)==4 && sizeof(fd_set)==128 && FD_SETSIZE==1024, "ARM64 socket public ABI");
static int native_flags(int flags) {
    int result=flags&7;
    if (flags&8) result|=AI_NUMERICSERV;
    if (flags&0x100) result|=AI_ALL;
    if (flags&0x800) result|=AI_V4MAPPED;
    if (flags&0x400) result|=AI_ADDRCONFIG;
    return result;
}
static int guest_flags(int flags) {
    return (flags&7) | ((flags&AI_NUMERICSERV)?8:0) | ((flags&AI_ALL)?0x100:0) |
        ((flags&AI_V4MAPPED)?0x800:0) | ((flags&AI_ADDRCONFIG)?0x400:0);
}
static int guest_error(int code) {
    switch(code) {
    case 0:return 0; case EAI_ADDRFAMILY:return 1; case EAI_AGAIN:return 2;
    case EAI_BADFLAGS:return 3; case EAI_FAIL:return 4; case EAI_FAMILY:return 5;
    case EAI_MEMORY:return 6; case EAI_NODATA:return 7; case EAI_NONAME:return 8;
    case EAI_SERVICE:return 9; case EAI_SOCKTYPE:return 10; case EAI_SYSTEM:return 11;
    case EAI_OVERFLOW:return 14; default:return 4;
    }
}
static int addresses(const char *node,const char *service,const GuestAddrInfo *hints,GuestAddrInfo **result) {
    if (!result) return 12;
    *result=NULL;
    struct addrinfo native_hints={0},*native=NULL;
    if (hints) {
        // AI_V4MAPPED_CFG is an obsolete Android-only kernel probe; reject it
        // rather than silently changing resolution. Known flags are converted.
        if (hints->flags&~(7|8|0x100|0x400|0x800)) return 3;
        native_hints.ai_flags=native_flags(hints->flags);
        native_hints.ai_family=hints->family; native_hints.ai_socktype=hints->type;
        native_hints.ai_protocol=hints->protocol;
    }
    int code=getaddrinfo(node,service,hints ? &native_hints : NULL,&native);
    if (code) return guest_error(code);
    size_t count=0;
    for (struct addrinfo *p=native;p;p=p->ai_next) ++count;
    if (!count || count>(SIZE_MAX-sizeof(Results))/sizeof(GuestAddrInfo)) {
        freeaddrinfo(native); return 6;
    }
    Results *owned=malloc(sizeof(*owned)+count*sizeof(GuestAddrInfo));
    if (!owned) { freeaddrinfo(native); return 6; }
    owned->native=native;
    size_t index=0;
    for (struct addrinfo *p=native;p;p=p->ai_next,++index) {
        owned->nodes[index]=(GuestAddrInfo){guest_flags(p->ai_flags),p->ai_family,p->ai_socktype,
            p->ai_protocol,p->ai_addrlen,p->ai_canonname,p->ai_addr,
            p->ai_next ? &owned->nodes[index+1] : NULL};
    }
    *result=owned->nodes;
    return 0;
}
static int reverse_address(const struct sockaddr *address,socklen_t length,char *host,socklen_t host_length,
    char *service,socklen_t service_length,int flags) {
    // Android NI_* follows BSD's numbering, unlike GNU.
    if (flags&~31) return 3;
    int native=((flags&1)?NI_NOFQDN:0) | ((flags&2)?NI_NUMERICHOST:0) |
        ((flags&4)?NI_NAMEREQD:0) | ((flags&8)?NI_NUMERICSERV:0) | ((flags&16)?NI_DGRAM:0);
    return guest_error(getnameinfo(address,length,host,host_length,service,service_length,native));
}
static const char *error_message(int error) {
    static const int native[]={0,EAI_ADDRFAMILY,EAI_AGAIN,EAI_BADFLAGS,EAI_FAIL,EAI_FAMILY,
        EAI_MEMORY,EAI_NODATA,EAI_NONAME,EAI_SERVICE,EAI_SOCKTYPE,EAI_SYSTEM,EAI_BADFLAGS,EAI_SERVICE,EAI_OVERFLOW};
    return (unsigned)error<sizeof(native)/sizeof(native[0]) ? gai_strerror(native[error]) : "Unknown address error";
}
static void release_addresses(GuestAddrInfo *result) {
    // Accept the head returned by this service, not native/foreign addrinfo.
    if (!result) return;
    Results *owned=(Results *)((char *)result - offsetof(Results,nodes));
    freeaddrinfo(owned->native); free(owned);
}
static void check_fd(int fd,size_t size) {
    if ((unsigned)fd>=FD_SETSIZE || size<sizeof(fd_set)) {
        fputs("Bionic fd_set bounds failure\n",stderr); abort();
    }
}
static int fd_isset(int fd,const fd_set *set,size_t size) { check_fd(fd,size); return FD_ISSET(fd,set); }
static void fd_set_checked(int fd,fd_set *set,size_t size) { check_fd(fd,size); FD_SET(fd,set); }
static void fd_clear_checked(int fd,fd_set *set,size_t size) { check_fd(fd,size); FD_CLR(fd,set); }
static const RudroidSymbol symbols[] = {
    RUDROID_SYMBOL("getnameinfo", reverse_address) RUDROID_SYMBOL("gai_strerror", error_message)
    RUDROID_SYMBOL("__get_h_errno", __h_errno_location)
    RUDROID_SYMBOL("getaddrinfo", addresses) RUDROID_SYMBOL("freeaddrinfo", release_addresses)
    RUDROID_SYMBOL("__FD_ISSET_chk", fd_isset) RUDROID_SYMBOL("__FD_SET_chk", fd_set_checked) RUDROID_SYMBOL("__FD_CLR_chk", fd_clear_checked)
    RUDROID_SYMBOL("process_vm_readv", process_vm_readv)
    RUDROID_SYMBOL("writev", writev)
    RUDROID_SYMBOL("socket", socket)
    RUDROID_SYMBOL("socketpair", socketpair)
    RUDROID_SYMBOL("accept", accept)
    RUDROID_SYMBOL("bind", bind)
    RUDROID_SYMBOL("connect", connect)
    RUDROID_SYMBOL("listen", listen)
    RUDROID_SYMBOL("shutdown", shutdown)
    RUDROID_SYMBOL("getsockname", getsockname)
    RUDROID_SYMBOL("getpeername", getpeername)
    RUDROID_SYMBOL("getsockopt", getsockopt)
    RUDROID_SYMBOL("setsockopt", setsockopt)
    RUDROID_SYMBOL("send", send)
    RUDROID_SYMBOL("recv", recv)
    RUDROID_SYMBOL("sendto", sendto)
    RUDROID_SYMBOL("recvfrom", recvfrom)
    RUDROID_SYMBOL("sendmsg", sendmsg)
    RUDROID_SYMBOL("recvmsg", recvmsg)
    RUDROID_SYMBOL("select", select)
    RUDROID_SYMBOL("inet_addr", inet_addr)
    RUDROID_SYMBOL("inet_ntop", inet_ntop)
    RUDROID_SYMBOL("inet_pton", inet_pton)
    RUDROID_SYMBOL("if_nametoindex", if_nametoindex)
    RUDROID_SYMBOL("gethostbyname", gethostbyname)
    RUDROID_SYMBOL("gethostbyaddr", gethostbyaddr)
};
RUDROID_SYMBOL_PROVIDER(network, symbols)
