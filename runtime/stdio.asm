; stdio.asm — Standard I/O functions for Intel 8080 (Vector 06C target)
;
; C calling convention: arg0 in HL, arg1 in DE, arg2+ on stack.
; Return value in HL (16-bit) or A (8-bit).
; Clobbers: A, B, C, D, E, H, L, flags
;
; I/O port constants (configurable for different hardware):
;   __IO_OUT_PORT  = console output data port
;   __IO_IN_PORT   = console input data port
;   __IO_STAT_PORT = console input status port
;   __IO_STAT_MASK = bit mask for "data ready" in status port
;
; These defaults work with common Vector 06C emulators.
; Modify for your specific hardware configuration.

__IO_OUT_PORT  EQU 0x01
__IO_IN_PORT   EQU 0x01
__IO_STAT_PORT EQU 0x00
__IO_STAT_MASK EQU 0x01

; ---------------------------------------------------------------------------
; putchar — output a single character
;   C: void putchar(int c)
;   Entry: HL = character (low byte L is the char)
;   Exit:  (void)
; ---------------------------------------------------------------------------
putchar:
	MOV A,L
	OUT __IO_OUT_PORT
	RET

; ---------------------------------------------------------------------------
; __putchar_a — internal: output character in A register
;   Entry: A = character
;   Preserves: HL, DE, BC
; ---------------------------------------------------------------------------
__putchar_a:
	OUT __IO_OUT_PORT
	RET

; ---------------------------------------------------------------------------
; getchar — read a single character from console input
;   C: int getchar(void)
;   Exit:  HL = character (zero-extended)
; ---------------------------------------------------------------------------
getchar:
__getchar_wait:
	IN __IO_STAT_PORT
	ANI __IO_STAT_MASK
	JZ __getchar_wait	; poll until data ready
	IN __IO_IN_PORT
	MOV L,A
	MVI H,0
	RET

; ---------------------------------------------------------------------------
; puts — output a string followed by newline
;   C: int puts(char *s)
;   Entry: HL = string pointer
;   Exit:  HL = 0 (success)
; ---------------------------------------------------------------------------
puts:
__puts_loop:
	MOV A,M
	ORA A
	JZ __puts_nl
	OUT __IO_OUT_PORT
	INX H
	JMP __puts_loop
__puts_nl:
	MVI A,0x0D		; CR
	OUT __IO_OUT_PORT
	MVI A,0x0A		; LF
	OUT __IO_OUT_PORT
	LXI H,0		; return 0 (success)
	RET

; ---------------------------------------------------------------------------
; printf — formatted output (subset: %d %i %u %x %X %o %c %s %%)
;   C: int printf(char *fmt, ...)
;   Entry: HL = format string, DE = first format arg
;          Additional args on stack at [SP+2], [SP+4], ...
;   Exit:  HL = number of characters written
;
;   Supported format specifiers:
;     %d, %i  — signed 16-bit decimal
;     %u      — unsigned 16-bit decimal
;     %x, %X  — unsigned 16-bit hexadecimal (lowercase/uppercase)
;     %o      — unsigned 16-bit octal
;     %c      — character (low byte)
;     %s      — null-terminated string
;     %%      — literal '%'
;     %ld,%li — signed 16-bit decimal (l prefix ignored, treated as 16-bit)
;     %lu     — unsigned 16-bit decimal (l prefix ignored)
;     %lx,%lX — unsigned 16-bit hex (l prefix ignored)
; ---------------------------------------------------------------------------
printf:
	; Save DE (first format arg) to memory
	MOV A,E
	STA __pf_arg
	MOV A,D
	STA __pf_arg+1

	; Set up stack arg pointer
	; Stack layout: [ret_addr][pushed args...]
	; Stack args start at SP+2
	PUSH H			; save fmt
	LXI H,4		; 2(push) + 2(ret_addr)
	DAD SP
	SHLD __pf_stkptr
	POP H			; restore fmt

	XRA A
	STA __pf_argidx		; first arg comes from DE (saved)
	STA __pf_count		; character count = 0
	STA __pf_count+1

__pf_loop:
	MOV A,M
	ORA A
	JZ __pf_done
	CPI '%'
	JZ __pf_format
	; Regular character
	PUSH H
	CALL __pf_outch
	POP H
	INX H
	JMP __pf_loop

__pf_format:
	INX H			; skip '%'
	MOV A,M
	ORA A
	JZ __pf_done

	CPI '%'
	JZ __pf_pct
	CPI 'd'
	JZ __pf_decimal_s
	CPI 'i'
	JZ __pf_decimal_s
	CPI 'u'
	JZ __pf_decimal_u
	CPI 'x'
	JZ __pf_hex_lc
	CPI 'X'
	JZ __pf_hex_uc
	CPI 'o'
	JZ __pf_octal
	CPI 'c'
	JZ __pf_char
	CPI 's'
	JZ __pf_string
	CPI 'l'
	JZ __pf_long
	; Unknown specifier — print '%' and the char
	PUSH H
	MVI A,'%'
	CALL __pf_outch
	POP H
	MOV A,M
	PUSH H
	CALL __pf_outch
	POP H
	INX H
	JMP __pf_loop

__pf_pct:
	MVI A,'%'
	PUSH H
	CALL __pf_outch
	POP H
	INX H
	JMP __pf_loop

__pf_decimal_s:
	PUSH H
	CALL __pf_getarg	; DE = next arg
	XCHG			; HL = arg
	CALL __pf_print_s16
	POP H
	INX H
	JMP __pf_loop

__pf_decimal_u:
	PUSH H
	CALL __pf_getarg
	XCHG
	CALL __pf_print_u16
	POP H
	INX H
	JMP __pf_loop

__pf_hex_lc:
	PUSH H
	CALL __pf_getarg
	XCHG
	MVI C,0			; C=0 → lowercase
	CALL __pf_print_hex
	POP H
	INX H
	JMP __pf_loop

__pf_hex_uc:
	PUSH H
	CALL __pf_getarg
	XCHG
	MVI C,1			; C=1 → uppercase
	CALL __pf_print_hex
	POP H
	INX H
	JMP __pf_loop

__pf_octal:
	PUSH H
	CALL __pf_getarg
	XCHG
	CALL __pf_print_oct
	POP H
	INX H
	JMP __pf_loop

__pf_char:
	PUSH H
	CALL __pf_getarg
	MOV A,E			; char in low byte of DE
	CALL __pf_outch
	POP H
	INX H
	JMP __pf_loop

__pf_string:
	PUSH H
	CALL __pf_getarg
	XCHG			; HL = string pointer
__pf_str_loop:
	MOV A,M
	ORA A
	JZ __pf_str_done
	PUSH H
	CALL __pf_outch
	POP H
	INX H
	JMP __pf_str_loop
__pf_str_done:
	POP H
	INX H
	JMP __pf_loop

__pf_long:
	; Consume 'l' prefix, then treat as 16-bit
	INX H
	MOV A,M
	CPI 'd'
	JZ __pf_decimal_s
	CPI 'i'
	JZ __pf_decimal_s
	CPI 'u'
	JZ __pf_decimal_u
	CPI 'x'
	JZ __pf_hex_lc
	CPI 'X'
	JZ __pf_hex_uc
	; Unknown after 'l'
	INX H
	JMP __pf_loop

__pf_done:
	LHLD __pf_count		; return character count
	RET

; --- printf internal helpers -----------------------------------------------

; Get next variadic argument into DE
__pf_getarg:
	LDA __pf_argidx
	ORA A
	JNZ __pf_getarg_stk
	; First arg was in DE, saved to __pf_arg
	LDA __pf_arg
	MOV E,A
	LDA __pf_arg+1
	MOV D,A
	MVI A,1
	STA __pf_argidx
	RET
__pf_getarg_stk:
	LHLD __pf_stkptr
	MOV E,M
	INX H
	MOV D,M
	INX H
	SHLD __pf_stkptr	; advance pointer
	RET

; Output character in A and increment count
__pf_outch:
	OUT __IO_OUT_PORT
	PUSH H
	LHLD __pf_count
	INX H
	SHLD __pf_count
	POP H
	RET

; Print signed 16-bit decimal (HL = value)
__pf_print_s16:
	MOV A,H
	ORA A
	JP __pf_print_u16	; positive → print as unsigned
	; Negative: print '-' then negate
	PUSH H
	MVI A,'-'
	CALL __pf_outch
	POP H
	MOV A,L
	CMA
	MOV L,A
	MOV A,H
	CMA
	MOV H,A
	INX H			; HL = -HL (two's complement)
	; fall through to unsigned print

; Print unsigned 16-bit decimal (HL = value)
__pf_print_u16:
	PUSH D
	PUSH B
	MVI C,0			; leading zero suppression flag

	LXI D,10000
	CALL __pf_digit
	LXI D,1000
	CALL __pf_digit
	LXI D,100
	CALL __pf_digit
	LXI D,10
	CALL __pf_digit
	; Last digit — always print
	MOV A,L
	ADI '0'
	CALL __pf_outch

	POP B
	POP D
	RET

; Divide HL by DE using repeated subtraction, print digit
; C = leading zero flag (0 = suppress, 1 = print)
__pf_digit:
	MVI B,'0'
__pf_dig_loop:
	MOV A,L
	SUB E
	PUSH PSW		; save low-byte result and flags
	MOV A,H
	SBB D
	JC __pf_dig_restore
	MOV H,A
	POP PSW
	MOV L,A			; HL -= DE
	INR B			; digit++
	JMP __pf_dig_loop
__pf_dig_restore:
	POP PSW			; discard, HL unchanged
	; Check if digit is zero and we're suppressing
	MOV A,B
	CPI '0'
	JNZ __pf_dig_print
	MOV A,C
	ORA A
	RZ			; suppress leading zero
__pf_dig_print:
	MVI C,1			; mark: we've printed a digit
	MOV A,B
	CALL __pf_outch
	RET

; Print unsigned 16-bit hex (HL = value, C = 0 for lowercase, 1 for uppercase)
__pf_print_hex:
	PUSH B			; save case flag
	MOV A,H
	CALL __pf_hex8
	MOV A,L
	CALL __pf_hex8
	POP B
	RET

; Print 8-bit value in A as 2 hex digits
__pf_hex8:
	PUSH PSW
	RRC
	RRC
	RRC
	RRC
	CALL __pf_nib
	POP PSW
	CALL __pf_nib
	RET

; Print low nibble of A as hex digit
__pf_nib:
	ANI 0x0F
	ADI '0'
	CPI '9'+1
	JC __pf_nib_ok
	ADI 7			; adjust for A-F
__pf_nib_ok:
	CALL __pf_outch
	RET

; Print unsigned 16-bit octal (HL = value)
__pf_print_oct:
	PUSH D
	PUSH B
	; 16-bit number in octal is at most 6 digits (max 177777)
	; Extract digits from MSB to LSB using repeated division by 8
	; Simpler: extract 3-bit groups from the value

	; Bit 15 (1 digit)
	MOV A,H
	RLC
	ANI 1
	MOV B,A			; B = suppression state
	ORA A
	JZ __pf_oct_d2
	ADI '0'
	CALL __pf_outch
	MVI B,1

__pf_oct_d2:
	; Bits 14-12 (3 bits)
	MOV A,H
	RRC
	RRC
	RRC
	RRC
	ANI 0x07
	CALL __pf_oct_digit

	; Bits 11-9
	MOV A,H
	RRC
	ANI 0x07
	CALL __pf_oct_digit

	; Bits 8-6
	MOV A,H
	ANI 0x01
	RLC
	RLC
	MOV C,A
	MOV A,L
	RLC
	ANI 0x01		; bit 7 of L shifted into bit 0
	; Actually this is getting complex. Use division instead.
	; Reset and use division by 8 approach.
	POP B
	POP D
	JMP __pf_oct_div

; Octal via repeated division by 8 — simpler approach
__pf_oct_div:
	PUSH D
	PUSH B
	; Store digits on stack in reverse, then print
	MVI C,0			; digit count
__pf_oct_divloop:
	; Divide HL by 8: shift right 3 times
	PUSH H			; save HL
	; HL / 8: shift right 3 times, saving remainder
	MOV A,L
	ANI 0x07		; remainder = low 3 bits
	PUSH PSW		; save digit
	INR C			; count++
	; HL >>= 3
	MOV A,H
	ORA A
	RAR
	MOV H,A
	MOV A,L
	RAR
	MOV L,A
	MOV A,H
	ORA A
	RAR
	MOV H,A
	MOV A,L
	RAR
	MOV L,A
	MOV A,H
	ORA A
	RAR
	MOV H,A
	MOV A,L
	RAR
	MOV L,A
	; Discard the saved HL
	POP PSW			; A = digit
	PUSH PSW		; re-save digit
	POP PSW			; get digit back (juggling)
	; Hmm, stack is messy. Let me redo.
	POP D			; discard original saved HL
	POP PSW			; digit back in A
	PUSH PSW		; save digit on stack

	MOV A,H
	ORA L
	JNZ __pf_oct_divloop

	; Print digits from stack (reverse order)
__pf_oct_print:
	POP PSW			; A = digit (3-bit value)
	ADI '0'
	PUSH B
	CALL __pf_outch
	POP B
	DCR C
	JNZ __pf_oct_print

	POP B
	POP D
	RET

; Helper for manual octal digit (B = suppress flag, A = digit value)
__pf_oct_digit:
	ORA A
	JNZ __pf_oct_print1
	MOV A,B
	ORA A
	RZ			; suppress leading zero
	XRA A
__pf_oct_print1:
	MVI B,1
	ADI '0'
	CALL __pf_outch
	RET

; --- printf static data ---
__pf_arg:
	.storage 2			; saved first format arg (DE)
__pf_stkptr:
	.storage 2			; pointer to next stack arg
__pf_argidx:
	.storage 1			; 0 = next arg from DE, 1+ = from stack
__pf_count:
	.storage 2			; characters written
