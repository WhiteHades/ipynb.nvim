; extends

; Keep prose spelling, but do not treat injected code as English. Python's
; higher-priority @spell captures still apply to comments and docstrings.
((fenced_code_block) @nospell
  (#set! priority 90))
