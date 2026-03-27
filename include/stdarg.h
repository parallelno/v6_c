#ifndef _STDARG_H
#define _STDARG_H

typedef char* va_list;

#define va_start(ap, last) ((ap) = (va_list)__builtin_va_start())
#define va_arg(ap, type)   (*(type*)((ap) += sizeof(type), (ap) - sizeof(type)))
#define va_end(ap)         ((void)0)

#endif /* _STDARG_H */
