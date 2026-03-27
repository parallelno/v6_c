/* stdlib.h — standard library declarations for v6c */
#ifndef _STDLIB_H
#define _STDLIB_H

#ifndef _SIZE_T_DEFINED
#define _SIZE_T_DEFINED
typedef unsigned int size_t;
#endif

#define NULL ((void *)0)

#define RAND_MAX 32767
#define EXIT_SUCCESS 0
#define EXIT_FAILURE 1

/* Numeric conversions */
int atoi(char *s);
int abs(int x);

/* Pseudo-random numbers */
int rand(void);
void srand(unsigned int seed);

/* Memory allocation */
void *malloc(size_t size);
void free(void *ptr);

#endif
