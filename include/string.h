/* string.h — string function declarations for v6c */
#ifndef _STRING_H
#define _STRING_H

void *memcpy(void *dest, void *src, unsigned int n);
void *memset(void *dest, int c, unsigned int n);
unsigned int strlen(char *s);
int strcmp(char *s1, char *s2);

#endif
