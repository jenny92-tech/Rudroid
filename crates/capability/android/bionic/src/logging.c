// INPUT:  Android liblog varargs and the process-pinned HostApi log callback
// OUTPUT:  rudroid_bionic_logging_symbol/_symbol_name table and one native callback installation boundary
// POS:    formats on a bounded stack buffer and forwards to the same core log service
#define _GNU_SOURCE
#include <errno.h>
#include <stdarg.h>
#include <stdatomic.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include "symbol_table.h"
typedef void (*Logger)(const char *,const char *);
static _Atomic(Logger) logger;
static _Atomic int diagnostics;
static _Atomic int minimum_priority=1; // ANDROID_LOG_DEFAULT, host default INFO.
enum { PRINT_CAPACITY=1024, DIAGNOSTIC_ERROR_CAPACITY=4096, TAG_OVERHEAD=128 };
static int extended_error(int priority) {
    return (priority==6 || priority==7) && atomic_load_explicit(&diagnostics,memory_order_relaxed);
}
static int get_minimum(void) { return atomic_load_explicit(&minimum_priority,memory_order_relaxed); }
static int set_minimum(int priority) { return atomic_exchange_explicit(&minimum_priority,priority,memory_order_relaxed); }
static int loggable(int priority,const char *tag,int default_priority) {
    (void)tag; (void)default_priority;
    int minimum=get_minimum();
    minimum=minimum==1 ? 4 : minimum;
    if (!atomic_load_explicit(&diagnostics,memory_order_relaxed) && minimum<6) minimum=6;
    return priority >= minimum;
}
void rudroid_bionic_install_logger(Logger next,int debug) {
    atomic_store_explicit(&diagnostics,debug==1,memory_order_relaxed);
    atomic_store_explicit(&logger,next,memory_order_release);
}
static int emit(const char *tag,const char *message,int failure,int extended) {
    if (!message) return -EINVAL;
    if (!tag) tag="<null>";
    int saved=errno;
    Logger sink=atomic_load_explicit(&logger,memory_order_acquire);
    if (sink && failure) {
        // Keep the tag wrapper from truncating a diagnostic stack a second
        // time. Both capacities remain bounded; ordinary calls and asserts
        // retain their original stack/text limits.
        char ordinary[PRINT_CAPACITY+TAG_OVERHEAD];
        size_t capacity=DIAGNOSTIC_ERROR_CAPACITY+TAG_OVERHEAD;
        char *large=extended ? malloc(capacity) : NULL;
        char *buffer=large ? large : ordinary;
        snprintf(buffer,large ? capacity : sizeof(ordinary),"[%s] %s",tag,message);
        sink("ERROR",buffer);
        free(large);
    } else if (sink) sink(tag,message);
    else fprintf(stderr,"%s[%s] %s\n",failure ? "[ERROR] " : "",tag,message); // Standalone provider before loader init.
    errno=saved; return 1;
}
static int write_log(int priority,const char *tag,const char *message) {
    if (!loggable(priority,tag,2)) return -EPERM;
    return emit(tag,message,priority>=6,extended_error(priority));
}
static int print_list(int priority,const char *tag,const char *format,va_list arguments) {
    if (!loggable(priority,tag,2)) return -EPERM;
    if (!format) return -EINVAL;
    int saved=errno;
    // Choose before consuming arguments: a va_list cannot be retried without
    // copying it. Only explicitly diagnostic ERROR/FATAL text grows.
    char ordinary[PRINT_CAPACITY];
    char *large=extended_error(priority) ? malloc(DIAGNOSTIC_ERROR_CAPACITY) : NULL;
    char *buffer=large ? large : ordinary;
    int length=vsnprintf(buffer,large ? DIAGNOSTIC_ERROR_CAPACITY : sizeof(ordinary),format,arguments);
    int result=length<0 ? -EINVAL : write_log(priority,tag,buffer);
    free(large);
    errno=saved; return result;
}
static int print_log(int priority,const char *tag,const char *format,...) {
    va_list arguments; va_start(arguments,format);
    int result=print_list(priority,tag,format,arguments);
    va_end(arguments); return result;
}
static int buffer_write(int id,int priority,const char *tag,const char *message) {
    (void)id; return write_log(priority,tag,message);
}
static int buffer_print(int id,int priority,const char *tag,const char *format,...) {
    (void)id; va_list arguments; va_start(arguments,format);
    int result=print_list(priority,tag,format,arguments);
    va_end(arguments); return result;
}
static void assert_log(const char *condition,const char *tag,const char *format,...) {
    char buffer[1024]={0}; // Encoding errors must never expose uninitialized stack bytes.
    if (format) {
        va_list arguments; va_start(arguments,format);
        vsnprintf(buffer,sizeof(buffer),format,arguments); va_end(arguments);
    } else snprintf(buffer,sizeof(buffer),"Assertion failed: %s",condition ? condition : "");
    emit(tag,buffer,1,0); // Assertions keep their original bound and bypass the minimum-priority filter.
    abort();
}
static const RudroidSymbol symbols[] = {
    RUDROID_SYMBOL("__android_log_get_minimum_priority", get_minimum)
    RUDROID_SYMBOL("__android_log_set_minimum_priority", set_minimum)
    RUDROID_SYMBOL("__android_log_is_loggable", loggable)
    RUDROID_SYMBOL("__android_log_write", write_log) RUDROID_SYMBOL("__android_log_print", print_log)
    RUDROID_SYMBOL("__android_log_vprint", print_list) RUDROID_SYMBOL("__android_log_assert", assert_log)
    RUDROID_SYMBOL("__android_log_buf_write", buffer_write) RUDROID_SYMBOL("__android_log_buf_print", buffer_print)
};
RUDROID_SYMBOL_PROVIDER(logging, symbols)
