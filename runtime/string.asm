; string.asm — additional string functions for Intel 8080
;
; C calling convention: arg0 in HL, arg1 in DE, arg2+ on stack.
; Return value in HL (16-bit) or A (8-bit).
; Clobbers: A, B, C, D, E, H, L, flags

; ---------------------------------------------------------------------------
; memmove — copy n bytes, handles overlapping regions
;   C: void *memmove(void *dest, void *src, unsigned int n)
;   Entry: HL = dest, DE = src, n at [SP+2]
;   Exit:  HL = dest
; ---------------------------------------------------------------------------
memmove:
	; Read n from stack
	PUSH H			; save dest
	LXI H,4		; 2(push) + 2(ret_addr)
	DAD SP
	MOV C,M
	INX H
	MOV B,M			; BC = n
	POP H			; HL = dest

	MOV A,B
	ORA C
	JZ __memmove_ret	; n == 0

	PUSH H			; save dest for return

	; Determine direction: if dest > src, copy backward
	MOV A,L
	SUB E
	MOV A,H
	SBB D			; carry if HL < DE (dest < src)
	JC __memmove_fwd

	; dest >= src: check if they overlap (dest < src + n)
	; Always copy backward to be safe
	; Set HL to dest+n-1, DE to src+n-1
	PUSH B			; save count
	DCX B			; count - 1
	DAD B			; HL = dest + n - 1
	XCHG
	DAD B			; HL = src + n - 1
	XCHG			; DE = src+n-1, HL = dest+n-1
	POP B			; restore count
__memmove_bwd:
	LDAX D
	MOV M,A
	DCX H
	DCX D
	DCX B
	MOV A,B
	ORA C
	JNZ __memmove_bwd
	POP H			; return original dest
	RET

__memmove_fwd:
	LDAX D
	MOV M,A
	INX H
	INX D
	DCX B
	MOV A,B
	ORA C
	JNZ __memmove_fwd
	POP H			; return original dest
	RET

__memmove_ret:
	RET

; ---------------------------------------------------------------------------
; strcpy — copy string from src to dest
;   C: char *strcpy(char *dest, char *src)
;   Entry: HL = dest, DE = src
;   Exit:  HL = dest
; ---------------------------------------------------------------------------
strcpy:
	PUSH H			; save dest
__strcpy_loop:
	LDAX D
	MOV M,A
	ORA A
	JZ __strcpy_done
	INX H
	INX D
	JMP __strcpy_loop
__strcpy_done:
	POP H
	RET

; ---------------------------------------------------------------------------
; strncpy — copy up to n chars from src to dest, pad with zeros
;   C: char *strncpy(char *dest, char *src, unsigned int n)
;   Entry: HL = dest, DE = src, n at [SP+2]
;   Exit:  HL = dest
; ---------------------------------------------------------------------------
strncpy:
	; Read n from stack
	PUSH H			; save dest
	LXI H,4
	DAD SP
	MOV C,M
	INX H
	MOV B,M			; BC = n
	POP H			; HL = dest

	PUSH H			; save dest for return
	MOV A,B
	ORA C
	JZ __strncpy_done
__strncpy_loop:
	LDAX D
	MOV M,A
	ORA A
	JZ __strncpy_pad
	INX H
	INX D
	DCX B
	MOV A,B
	ORA C
	JNZ __strncpy_loop
	JMP __strncpy_done
__strncpy_pad:
	DCX B
	MOV A,B
	ORA C
	JZ __strncpy_done
	INX H
	MVI M,0
	JMP __strncpy_pad
__strncpy_done:
	POP H
	RET

; ---------------------------------------------------------------------------
; strcat — append src string to end of dest
;   C: char *strcat(char *dest, char *src)
;   Entry: HL = dest, DE = src
;   Exit:  HL = dest
; ---------------------------------------------------------------------------
strcat:
	PUSH H			; save dest
	PUSH D			; save src
	; Find end of dest
__strcat_find:
	MOV A,M
	ORA A
	JZ __strcat_copy
	INX H
	JMP __strcat_find
__strcat_copy:
	POP D			; restore src
__strcat_loop:
	LDAX D
	MOV M,A
	ORA A
	JZ __strcat_done
	INX H
	INX D
	JMP __strcat_loop
__strcat_done:
	POP H			; return original dest
	RET

; ---------------------------------------------------------------------------
; strncat — append up to n chars from src to dest
;   C: char *strncat(char *dest, char *src, unsigned int n)
;   Entry: HL = dest, DE = src, n at [SP+2]
;   Exit:  HL = dest
; ---------------------------------------------------------------------------
strncat:
	; Read n from stack
	PUSH H
	LXI H,4
	DAD SP
	MOV C,M
	INX H
	MOV B,M			; BC = n
	POP H

	PUSH H			; save dest for return
	PUSH D			; save src
	PUSH B			; save count

	; Find end of dest
__strncat_find:
	MOV A,M
	ORA A
	JZ __strncat_setup
	INX H
	JMP __strncat_find
__strncat_setup:
	POP B			; restore count
	POP D			; restore src
	MOV A,B
	ORA C
	JZ __strncat_term
__strncat_loop:
	LDAX D
	ORA A
	JZ __strncat_term
	MOV M,A
	INX H
	INX D
	DCX B
	MOV A,B
	ORA C
	JNZ __strncat_loop
__strncat_term:
	MVI M,0			; null terminate
	POP H			; return original dest
	RET

; ---------------------------------------------------------------------------
; strncmp — compare up to n characters
;   C: int strncmp(char *s1, char *s2, unsigned int n)
;   Entry: HL = s1, DE = s2, n at [SP+2]
;   Exit:  HL = <0, 0, or >0
; ---------------------------------------------------------------------------
strncmp:
	; Read n from stack into memory counter
	PUSH H			; save s1
	LXI H,4
	DAD SP
	MOV A,M
	STA __strncmp_cnt
	INX H
	MOV A,M
	STA __strncmp_cnt+1
	POP H			; HL = s1

__strncmp_loop:
	; Check count
	LDA __strncmp_cnt
	MOV C,A
	LDA __strncmp_cnt+1
	MOV B,A
	MOV A,B
	ORA C
	JZ __strncmp_eq

	; Decrement count
	MOV A,C
	SUI 1
	STA __strncmp_cnt
	MOV A,B
	SBI 0
	STA __strncmp_cnt+1

	; Compare chars
	LDAX D			; A = *s2
	MOV B,A			; B = *s2
	MOV A,M			; A = *s1
	CMP B
	JNZ __strncmp_diff
	ORA A			; null terminator?
	JZ __strncmp_eq
	INX H
	INX D
	JMP __strncmp_loop

__strncmp_diff:
	; A = *s1, B = *s2
	SUB B			; A = *s1 - *s2
	MOV L,A
	MVI H,0
	ORA A
	RP
	MVI H,0xFF		; sign-extend negative
	RET

__strncmp_eq:
	LXI H,0
	RET

__strncmp_cnt:
	.storage 2

; ---------------------------------------------------------------------------
; strchr — find first occurrence of char c in string s
;   C: char *strchr(char *s, int c)
;   Entry: HL = s, DE = c (low byte E is the char)
;   Exit:  HL = pointer to found char, or 0 (NULL)
; ---------------------------------------------------------------------------
strchr:
__strchr_loop:
	MOV A,M
	CMP E			; compare with target char
	JZ __strchr_found
	ORA A			; null terminator?
	JZ __strchr_notfound
	INX H
	JMP __strchr_loop
__strchr_found:
	RET			; HL already points to the char
__strchr_notfound:
	; Check if searching for '\0'
	MOV A,E
	ORA A
	JZ __strchr_found_null
	LXI H,0		; return NULL
	RET
__strchr_found_null:
	RET			; HL points to the null terminator

; ---------------------------------------------------------------------------
; strrchr — find last occurrence of char c in string s
;   C: char *strrchr(char *s, int c)
;   Entry: HL = s, DE = c (low byte E is the char)
;   Exit:  HL = pointer to last found char, or 0 (NULL)
; ---------------------------------------------------------------------------
strrchr:
	LXI B,0		; BC = last found position (0 = not found)
__strrchr_loop:
	MOV A,M
	CMP E
	JNZ __strrchr_skip
	MOV B,H
	MOV C,L		; save this position
__strrchr_skip:
	MOV A,M
	ORA A
	JZ __strrchr_done
	INX H
	JMP __strrchr_loop
__strrchr_done:
	; If searching for '\0', check the terminator
	MOV A,E
	ORA A
	JNZ __strrchr_result
	MOV B,H
	MOV C,L		; point to the null terminator
__strrchr_result:
	MOV H,B
	MOV L,C		; HL = last found (or 0 if never found)
	RET

; ---------------------------------------------------------------------------
; memcmp — compare n bytes of memory
;   C: int memcmp(void *s1, void *s2, unsigned int n)
;   Entry: HL = s1, DE = s2, n at [SP+2]
;   Exit:  HL = <0, 0, or >0
; ---------------------------------------------------------------------------
memcmp:
	; Read n from stack
	PUSH H			; save s1
	LXI H,4
	DAD SP
	MOV C,M
	INX H
	MOV B,M			; BC = n
	POP H			; HL = s1

	MOV A,B
	ORA C
	JZ __memcmp_eq		; n == 0, equal

__memcmp_loop:
	LDAX D			; A = *s2
	MOV B,A			; B = *s2 (note: this clobbers B in BC counter)
	; We need BC for count AND B for comparison. Use stack to preserve count.
	PUSH B			; save B (s2 byte) and C (count low)
	MOV A,M			; A = *s1
	POP B			; restore B,C
	CMP B			; compare *s1 with *s2 (B still has *s2)
	JNZ __memcmp_diff
	INX H
	INX D
	DCX B
	MOV A,B
	ORA C
	JNZ __memcmp_loop

__memcmp_eq:
	LXI H,0
	RET

__memcmp_diff:
	; A had result of comparison; recalculate
	MOV A,M			; A = *s1
	SUB B			; A = *s1 - *s2
	MOV L,A
	MVI H,0
	ORA A
	RP
	MVI H,0xFF
	RET
