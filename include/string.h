/* string.h — string function declarations for v6c */
#ifndef _STRING_H
#define _STRING_H

#ifndef _SIZE_T_DEFINED
#define _SIZE_T_DEFINED
typedef unsigned int size_t;
#endif

#define NULL ((void *)0)

/* Copying */
void *memcpy(void *dest, void *src, size_t n);
void *memmove(void *dest, void *src, size_t n);
char *strcpy(char *dest, char *src);
char *strncpy(char *dest, char *src, size_t n);

/* Concatenation */
char *strcat(char *dest, char *src);
char *strncat(char *dest, char *src, size_t n);

/* Comparison */
int memcmp(void *s1, void *s2, size_t n);
int strcmp(char *s1, char *s2);
int strncmp(char *s1, char *s2, size_t n);

/* Searching */
char *strchr(char *s, int c);
char *strrchr(char *s, int c);

/* Other */
size_t strlen(char *s);
void *memset(void *dest, int c, size_t n);

#endif
