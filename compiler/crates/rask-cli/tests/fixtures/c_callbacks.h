#ifndef C_CALLBACKS_H
#define C_CALLBACKS_H
typedef int (*cmp_fn)(int, int);
int call_cmp(cmp_fn f, int a, int b);
typedef void (*visit_fn)(void *userdata, int value);
void each(const int *xs, int n, visit_fn visit, void *userdata);
typedef int (*fold_fn)(int acc, int value, void *ctx);
int fold(const int *xs, int n, fold_fn f, void *ctx, int start);
int sum_ints(int count, ...);
double avg_doubles(int count, ...);
#endif
