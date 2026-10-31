#include "c_callbacks.h"
#include <stdarg.h>
int call_cmp(cmp_fn f, int a, int b) { return f(a, b); }
void each(const int *xs, int n, visit_fn visit, void *userdata) { for (int i = 0; i < n; i++) visit(userdata, xs[i]); }
int fold(const int *xs, int n, fold_fn f, void *ctx, int start) { int acc = start; for (int i = 0; i < n; i++) acc = f(acc, xs[i], ctx); return acc; }
int sum_ints(int count, ...) { va_list ap; va_start(ap, count); int s = 0; for (int i = 0; i < count; i++) s += va_arg(ap, int); va_end(ap); return s; }
double avg_doubles(int count, ...) { va_list ap; va_start(ap, count); double s = 0; for (int i = 0; i < count; i++) s += va_arg(ap, double); va_end(ap); return s / count; }
