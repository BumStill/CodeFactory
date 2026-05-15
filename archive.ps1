# CodeFactory daily GitHub archive
Set-Location D:\CodeFactory

$date = Get-Date -Format "yyyy-MM-dd"
$msg  = "chore: daily archive $date"

git add -A

# Only commit if there are staged changes
$status = git diff --cached --name-only
if ($status) {
    git commit -m $msg
    git push origin main
    Write-Host "Archived: $msg"
} else {
    Write-Host "No changes on $date, skipping commit."
}
