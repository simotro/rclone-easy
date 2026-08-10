; Rclone Easy resta in esecuzione in background dopo aver chiuso la
; finestra (icona nella tray, "Esci" dal suo menu per uscire davvero, vedi
; hide_instead_of_close in lib.rs) — comportamento voluto, non un bug. Per
; questo un utente che lancia l'installer per aggiornare l'app la trova
; quasi sempre ancora aperta, con "rclone.exe" (il demone rcd sidecar)
; ancora in esecuzione: l'installer falliva con "Error opening file for
; writing" su rclone.exe, non riuscendo a sovrascriverlo (segnalato da
; Simone il 10/8/2026). Terminare entrambi i processi prima di copiare i
; file evita il blocco, indipendentemente da come l'utente li ha lasciati
; aperti.
;
; taskkill non fa fallire l'installazione se non trova il processo (nessun
; controllo sul codice di uscita): sicuro anche a prima installazione,
; quando nulla è ancora in esecuzione.
!macro NSIS_HOOK_PREINSTALL
  ExecWait 'taskkill /IM rclone-easy.exe /F'
  ExecWait 'taskkill /IM rclone.exe /F'
!macroend

; Stesso problema in disinstallazione: Windows non può cancellare un
; eseguibile ancora in uso.
!macro NSIS_HOOK_PREUNINSTALL
  ExecWait 'taskkill /IM rclone-easy.exe /F'
  ExecWait 'taskkill /IM rclone.exe /F'
!macroend
