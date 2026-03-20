.PHONY: dev dev-backend dev-frontend test test-backend test-frontend build clean

dev:
	@echo "启动 KooixMail 开发环境..."
	@make -j2 dev-backend dev-frontend

dev-backend:
	cd backend && cargo run

dev-frontend:
	cd frontend && npm run dev

test:
	@make test-backend
	@make test-frontend

test-backend:
	cd backend && cargo test

test-frontend:
	cd frontend && npx tsc --noEmit

build:
	cd backend && cargo build --release
	cd frontend && npm run build

clean:
	cd backend && cargo clean
	rm -rf frontend/dist
