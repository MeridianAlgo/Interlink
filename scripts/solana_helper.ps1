# InterLink Solana Setup Helper (Windows PowerShell)

$PROGRAM_ID = "AKzpc9tvxfhLjj5AantKizK2YZgSjoyhLmYqRZy6b8Lz"
$HUB_DIR = "contracts/solana/interlink-hub"

function Show-Help {
    Write-Host "InterLink Solana CLI Helper" -ForegroundColor Cyan
    Write-Host "---------------------------"
    Write-Host "1. build      - Build the Anchor program"
    Write-Host "2. test       - Run Anchor tests (localnet)"
    Write-Host "3. deploy     - Deploy to Devnet"
    Write-Host "4. airdrop    - Airdrop SOL on Devnet"
    Write-Host "5. balance    - Check wallet balance"
    Write-Host "---------------------------"
}

if ($args.Count -eq 0) {
    Show-Help
    exit
}

switch ($args[0]) {
    "build" {
        Write-Host "Building Solana Hub..." -ForegroundColor Yellow
        Set-Location $HUB_DIR
        anchor build
    }
    "test" {
        Write-Host "Running Integration Tests..." -ForegroundColor Yellow
        Set-Location $HUB_DIR
        anchor test
    }
    "deploy" {
        Write-Host "Deploying to Devnet..." -ForegroundColor Yellow
        Set-Location $HUB_DIR
        anchor deploy --provider.cluster devnet
    }
    "airdrop" {
        Write-Host "Airdropping 2 SOL..." -ForegroundColor Yellow
        solana airdrop 2 --url devnet
    }
    "balance" {
        solana balance --url devnet
    }
    Default {
        Write-Host "Unknown command: $($args[0])" -ForegroundColor Red
        Show-Help
    }
}
