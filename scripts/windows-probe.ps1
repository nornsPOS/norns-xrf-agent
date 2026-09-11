# Nur auf einem frischen Windows-Pruefrechner ausfuehren.
param([Parameter(Mandatory=$true)][string]$Programm)
$ErrorActionPreference = 'Stop'
$probeOrt = Join-Path $env:RUNNER_TEMP ('norns-xrf-' + [guid]::NewGuid())
New-Item -ItemType Directory -Path $probeOrt | Out-Null
$probeDb = Join-Path $probeOrt 'samplesummary.db'
@'
import sqlite3, sys
c=sqlite3.connect(sys.argv[1])
c.execute('CREATE TABLE summarys (KeyId INTEGER PRIMARY KEY, AppName TEXT, SampleName TEXT, MeasureTime INTEGER, InfoSaveFile TEXT, ResultContent TEXT)')
c.execute('INSERT INTO summarys VALUES (1,?,?,?,?,?)', ('AuAgX','synthetic',1780000000,'','<Result><Compo Name="Au" Fractal="58.5" /></Result>'))
c.commit()
c.close()
'@ | python - $probeDb
if ($LASTEXITCODE -ne 0) { throw 'Die Testdatenbank fehlt.' }
try {
    & $Programm --einrichten-still $probeDb
    if ($LASTEXITCODE -ne 0) { throw 'Die Einrichtung ist fehlgeschlagen.' }
    $codeDatei = Join-Path $env:ProgramData 'Norns\XrfAgent\botenwort.txt'
    $code = (Get-Content $codeDatei -Raw).Trim()
    function Read-Probe {
        $antwort = Invoke-RestMethod 'http://127.0.0.1:9614/messungen?seit=-1' -Headers @{'X-Norns-Bote'=$code}
        if ($antwort.protokoll -ne 2 -or !$antwort.quelleOk -or $antwort.messungen[0].goldPromille -ne 585) { throw 'Falsche Antwort des installierten Boten.' }
        return $antwort.sitzung
    }
    $vorher = Read-Probe
    Stop-ScheduledTask -TaskName 'Norns XRF Agent'
    $ende = (Get-Date).AddSeconds(10)
    do {
        $listener = Get-NetTCPConnection -LocalPort 9614 -State Listen -ErrorAction SilentlyContinue
        if (!$listener) { break }
        Start-Sleep -Milliseconds 100
    } while ((Get-Date) -lt $ende)
    if ($listener) { throw 'Die Aufgabe beendet den Ergebnisdienst nicht.' }
    Start-ScheduledTask -TaskName 'Norns XRF Agent'
    $ende = (Get-Date).AddSeconds(15)
    do {
        try { $nachher = Read-Probe; break } catch { Start-Sleep -Milliseconds 200 }
    } while ((Get-Date) -lt $ende)
    if (!$nachher -or $nachher -eq $vorher) { throw 'Der echte Aufgaben-Neustart fehlt.' }
    if ((Get-Content $codeDatei -Raw).Trim() -ne $code) { throw 'Der Verbindungscode wurde beim Neustart ersetzt.' }
    Write-Output 'Windows task: installed, authenticated measurement 585, restarted, same connection code.'
} finally {
    & $Programm --entfernen
    if ($LASTEXITCODE -ne 0) { Write-Error 'Die Testaufgabe konnte nicht entfernt werden.' }
}
