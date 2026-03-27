/* stdio.h — I/O function declarations for v6c */
#ifndef _STDIO_H
#define _STDIO_H

#define NULL ((void *)0)
#define EOF  (-1)

/* Character I/O */
void putchar(int c);
int getchar(void);

/* String I/O */
int puts(char *s);

/* Formatted I/O */
int printf(char *fmt, ...);

#endif
